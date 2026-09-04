//! Zellij lifecycle and rendering.

use std::collections::BTreeMap;
use zellij_tile::prelude::*;

use crate::state::{Grouping, State};
use crate::status::Status;
use crate::style::{chars, DIM_LEVEL};
use crate::util::truncate;
use crate::{content_width, host, ribbon, PANE_TITLE};

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.popup_on_waiting = configuration
            .get("popup_on_waiting")
            .map(|v| v != "false")
            .unwrap_or(true);
        self.discover = configuration.get("discover").map(|v| v != "false").unwrap_or(true);
        self.notifier.triggers = match configuration.get("notify") {
            Some(spec) => crate::notify::Triggers::parse(spec),
            None => crate::notify::Triggers::default(),
        };
        self.notifier.cooldown = configuration
            .get("notify_cooldown")
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(60.0);
        self.notifier.sound = configuration.get("notify_sound").map(|v| v == "true").unwrap_or(false);
        self.summary_path = configuration.get("summary_file").cloned().unwrap_or_default();

        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
            // Only the install screen needs this; it shells out to init.sh.
            PermissionType::RunCommands,
        ]);
        subscribe(&[
            EventType::PaneUpdate,
            EventType::TabUpdate,
            EventType::Key,
            EventType::Timer,
            EventType::PermissionRequestResult,
            EventType::RunCommandResult,
            // Carries the session name, which scopes the discovery scan.
            EventType::SessionUpdate,
            // A notification is redundant while the panel is already on screen.
            EventType::Visible,
        ]);
        set_selectable(true);

        self.rename_pane();
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            // Denial is not fatal, so this ignores the result: only the install
            // screen needs RunCommands, and it reports its own failures.
            Event::PermissionRequestResult(_) => {
                self.permissions_granted = true;
                // `run_command` only reaches the host after the grant; a refresh
                // fired from `load` is silently dropped.
                self.install.refresh();
                self.request_scan();
                self.detect_notifier();
                self.rename_pane();
                true
            }
            Event::Visible(visible) => {
                self.notifier.focused = visible;
                match visible {
                    true => self.clear_notified(),
                    // A search left open would greet the next open by silently
                    // swallowing keys into a prompt nobody remembers typing.
                    false => self.find.take().is_some(),
                }
            }
            Event::Timer(_) => self.on_tick(),
            Event::PaneUpdate(manifest) => {
                self.reconcile(manifest);
                // A pane appearing or closing is the cheapest signal that the
                // set of running agents may have changed.
                self.request_scan();
                true
            }
            Event::SessionUpdate(sessions, _) => {
                let live: Vec<String> = sessions
                    .iter()
                    .map(|s| crate::agent::sanitize_session(&s.name))
                    .collect();
                // The only place both spellings of a session name are known, so
                // the only place the addressing map can be built.
                self.session_names = sessions
                    .iter()
                    .map(|s| (crate::agent::sanitize_session(&s.name), s.name.clone()))
                    .collect();
                let changed = self.apply_sessions(live);
                let Some(name) = sessions
                    .iter()
                    .find(|s| s.is_current_session)
                    .map(|s| crate::agent::sanitize_session(&s.name))
                else {
                    return changed;
                };
                if self.session_name == name {
                    return changed;
                }
                self.session_name = name;
                self.request_scan();
                changed
            }
            Event::Key(key) => self.handle_key(key),
            Event::RunCommandResult(exit_code, stdout, stderr, context) => {
                let out = String::from_utf8_lossy(&stdout);
                let err = String::from_utf8_lossy(&stderr);
                if context.get(crate::install::CTX_KEY).map(String::as_str) == Some(crate::notify::CTX_DETECT) {
                    // A failed probe is recorded as `none` rather than left
                    // empty, so it is not retried on every permission event.
                    self.notifier.binary = match exit_code.unwrap_or(0) == 0 && !out.trim().is_empty() {
                        true => out.trim().to_string(),
                        false => "none".to_string(),
                    };
                    return false;
                }
                if context.get(crate::install::CTX_KEY).map(String::as_str) == Some(crate::discover::CTX_SCAN) {
                    self.scan_pending = false;
                    // A failed scan leaves the list exactly as it was: discovery
                    // is an enhancement, and hook-reported rows are the truth.
                    return exit_code.unwrap_or(0) == 0 && self.apply_scan_result(crate::discover::parse(&out));
                }
                // A cross-session action goes through the `zellij` binary, so
                // its failure is the plugin's only evidence that a kill or a
                // reply did not land. Saying nothing would leave the panel
                // claiming success while the agent keeps running.
                if let Some(kind) = context.get("kind").filter(|k| *k == "kill" || *k == "reply") {
                    // `zellij` exits 0 even when the session does not exist and
                    // reports it on stderr, so the exit code alone would miss
                    // precisely the failure this exists to catch.
                    let detail = err.lines().next().unwrap_or("").trim();
                    let failed = exit_code.unwrap_or(0) != 0 || !detail.is_empty();
                    if failed {
                        let what = match kind.as_str() {
                            "kill" => "kill",
                            _ => "reply",
                        };
                        self.action_error = Some(match detail.is_empty() {
                            true => format!("{} failed in the other session", what),
                            false => format!("{} failed: {}", what, crate::util::truncate(detail, 70)),
                        });
                        return true;
                    }
                    return false;
                }
                self.install.on_command_result(exit_code, &out, &err, &context)
            }
            _ => false,
        }
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        match pipe_message.name.as_str() {
            "agent-status" => self.handle_status(&pipe_message.args),
            "agent-label" => self.handle_label(&pipe_message.args),
            "agent-ask" => self.handle_ask(&pipe_message.args),
            _ => false,
        }
    }

    /// Never use `println!` here. Components position the cursor themselves via
    /// a DCS sequence, so a plain line lands wherever the last one left it and
    /// rows collide. Every element gets an explicit `y`.
    fn render(&mut self, rows: usize, cols: usize) {
        if !self.permissions_granted {
            print_text_with_coordinates(
                Text::new("zj-agent-mob needs permissions - press 'y' to grant"),
                0,
                0,
                None,
                None,
            );
            return;
        }

        let width = content_width(cols);

        if self.install.open {
            self.render_install(width);
            return;
        }
        if self.showing_setup() {
            self.render_setup(width);
            return;
        }
        if self.agents.is_empty() {
            self.render_empty(width);
            return;
        }
        self.render_list(rows, width);
    }
}

/// The boxed permission prompt shown under its agent's row. Indented to line up
/// with the detail line so it reads as belonging to that agent.
pub(crate) fn ask_rows(ask: &crate::state::Ask, cols: usize) -> Vec<Text> {
    const INDENT: &str = "        ";
    let inner = cols.saturating_sub(INDENT.len() + 4).clamp(8, 60);
    let bar = "\u{2500}".repeat(inner + 2);

    let head = truncate(&ask.tool_name, inner);
    let body = truncate(&ask.tool_arg, inner);
    let keys = "a approve    r reject    \u{21b5} jump to pane";

    let mut rows = vec![
        Text::new(format!("{}\u{250c}{}\u{2510}", INDENT, bar)).color_range(DIM_LEVEL, ..),
        Text::new(format!("{}\u{2502} {:<w$} \u{2502}", INDENT, head, w = inner)),
    ];
    if !body.is_empty() {
        // The command being approved is the one thing here that must not read
        // as chrome: it is what the user is actually deciding about.
        rows.push(Text::new(format!("{}\u{2502} {:<w$} \u{2502}", INDENT, body, w = inner)).error_color_range(..));
    }
    rows.push(
        Text::new(format!(
            "{}\u{2502} {:<w$} \u{2502}",
            INDENT,
            truncate(keys, inner),
            w = inner
        ))
        .color_range(DIM_LEVEL, ..),
    );
    rows.push(Text::new(format!("{}\u{2514}{}\u{2518}", INDENT, bar)).color_range(DIM_LEVEL, ..));
    rows
}

/// The one-line reply editor, indented under the agent it will be sent to. A
/// block cursor marks where typing lands, since Zellij owns the real one.
pub(crate) fn reply_row(text: &str, cols: usize) -> Text {
    const INDENT: &str = "      ";
    const PROMPT: &str = "\u{2514} reply: ";
    let room = cols.saturating_sub(INDENT.len() + chars(PROMPT) + 1);
    let shown = truncate(text, room);
    let line = format!("{}{}{}\u{2588}", INDENT, PROMPT, shown);
    let at = chars(INDENT)..chars(INDENT) + chars(PROMPT);
    Text::new(line).color_range(2, at)
}

/// The find prompt, footer-resident like a pending `g` count: the query, a
/// block cursor, and the keys that end the search.
pub(crate) fn find_prompt_row(query: &str, cols: usize) -> Text {
    let line = truncate(
        &format!("  /{}\u{2588}   \u{21b5} jump   esc cancel   ^j/^k move", query),
        cols,
    );
    // The highlight covers the slash, the query and the cursor, clamped so a
    // narrow pane cannot leave the range pointing past the truncated line.
    let end = (chars("  /") + chars(query) + 1).min(chars(&line)).max(2);
    Text::new(line).color_range(2, 2..end)
}

/// Which agent groups fit in `budget` rows, scrolled so `selected` is whole.
///
/// Groups are variable height (a detail line, a prompt box, a reply editor), so
/// the window is computed in rows rather than assumed to be one row per agent.
struct View {
    first: usize,
    count: usize,
    hidden_above: usize,
    hidden_below: usize,
}

fn viewport(groups: &[Vec<Text>], scroll: usize, selected: usize, budget: usize) -> View {
    let total = groups.len();
    let height = |i: usize| groups[i].len();
    let mut first = scroll.min(selected).min(total.saturating_sub(1));

    // Walk `first` forward until the selected group's last row fits. The `↑`/`↓`
    // markers cost a row each, so they are charged as the window is measured.
    loop {
        let mut used = usize::from(first > 0);
        let mut last = first;
        let mut fits = false;
        for i in first..total {
            let next = used + height(i) + usize::from(i + 1 < total);
            if next > budget {
                break;
            }
            used += height(i);
            last = i;
            fits = true;
        }
        if !fits || last >= selected || first >= selected {
            let count = match fits {
                true => last - first + 1,
                false => 0,
            };
            return View {
                first,
                count,
                hidden_above: first,
                hidden_below: total - first - count,
            };
        }
        first += 1;
    }
}

/// A group heading, carrying its member count so a collapsed-looking group is
/// still legible when the viewport cuts it off below.
fn group_header(name: &str, n: usize, width: usize) -> Text {
    let line = truncate(&format!("  {} ({})", name, n), width);
    Text::new(line).color_range(DIM_LEVEL, ..)
}

/// The affordance that stops a hidden row from being silently hidden.
fn more_row(n: usize, up: bool, width: usize) -> Text {
    let arrow = if up { "\u{2191}" } else { "\u{2193}" };
    let line = truncate(&format!("  {} {} more", arrow, n), width);
    Text::new(line).color_range(DIM_LEVEL, ..)
}

impl State {
    fn rename_pane(&self) {
        host::rename_own_pane(PANE_TITLE);
    }

    fn render_install(&self, width: usize) {
        let mut y = self.render_header("install", width);
        y = self.render_rows(self.install.list_items(), y);
        y = self.render_rule(y, width);
        y = self.render_notes(self.install.notes(), y, width);
        self.render_hints(ribbon::INSTALL_HINTS, y, width);
    }

    fn render_setup(&self, width: usize) {
        let mut y = self.render_header("setup", width);
        print_text_with_coordinates(
            Text::new("  Hooks are not installed, so no agent can report status.").color_range(DIM_LEVEL, ..),
            0,
            y,
            None,
            None,
        );
        y += 2;
        y = self.render_rows(self.install.setup_items(), y);
        y = self.render_rule(y, width);
        y = self.render_notes(self.install.notes(), y, width);
        self.render_hints(ribbon::SETUP_HINTS, y, width);
    }

    /// "No agents" is a claim the panel can only make once a scan has come back
    /// empty. Before that it has merely not been told about any.
    fn render_empty(&self, width: usize) {
        let subtitle = if self.scan_pending {
            "looking for agents"
        } else {
            "no agents in this session"
        };
        let y = self.render_header(subtitle, width);
        let rows = vec![
            Text::new("  Start claude or codex in a pane; hooks report status here.").color_range(DIM_LEVEL, ..),
            Text::new("  Press n to start one here, or i to check and install the hooks.").color_range(DIM_LEVEL, ..),
        ];
        self.render_rows(rows, y);
    }

    /// Returns the number of rows emitted, which must never exceed `rows`: a
    /// row printed past the bottom edge is clipped by the terminal, taking the
    /// footer and the key hints with it.
    fn render_list(&mut self, rows: usize, width: usize) -> usize {
        let (failed, waiting, working, done) = self.counts();
        // A zero failure count is omitted so the common case reads unchanged.
        let mut parts = Vec::new();
        if failed > 0 {
            parts.push((failed, "failed", Status::Failed));
        }
        parts.extend([
            (waiting, "waiting", Status::Waiting),
            (working, "working", Status::Working),
            (done, "done", Status::Done),
        ]);
        // Its own bucket rather than folded into one of the above: these agents
        // have a process and nothing else, so counting them as any real status
        // would put a number behind a claim the scan cannot make.
        let discovered = self.discovered_count();
        if discovered > 0 {
            parts.push((discovered, "found", Status::Discovered));
        }

        // Built up incrementally so each count's colour range tracks the digits
        // actually written; a two-digit count shifts everything after it.
        let mut head = "zj-agent-mob   ".to_string();
        let mut ranges = Vec::new();
        for (i, (n, label, status)) in parts.into_iter().enumerate() {
            if i > 0 {
                head.push_str(" \u{b7} ");
            }
            let digits = n.to_string();
            // Character offsets: the `\u{b7}` separator is multi-byte, so byte
            // offsets would drift right by one per separator already written.
            let range = chars(&head)..chars(&head) + digits.len();
            ranges.push((status.color_level(), status.is_error(), range));
            head.push_str(&digits);
            head.push(' ');
            head.push_str(label);
        }
        // Only when grouped: the default ordering needs no announcing. `s` is
        // in the footer either way, so this chip only names the current mode.
        let mut group_range = None;
        if self.grouping != Grouping::Urgency {
            let chip = format!("  [{} groups]", self.grouping.label());
            if chars(&head) + chars(&chip) <= width {
                let start = chars(&head);
                head.push_str(&chip);
                group_range = Some(start..start + chars(&chip));
            }
        }
        let head = ranges.into_iter().fold(Text::new(head), |t, (level, is_err, r)| {
            if is_err {
                t.error_color_range(r)
            } else {
                t.color_range(level, r)
            }
        });
        let head = match group_range {
            Some(r) => head.color_range(DIM_LEVEL, r),
            None => head,
        };
        print_text_with_coordinates(head, 0, 0, None, None);
        let mut y = self.render_rule(1, width);

        // An open find prompt narrows the list to its matches, in score order.
        // With no prompt this is the identity, so one code path renders both.
        let finding = self.find.is_some();
        let visible = self.find_matches();
        // While finding the marker follows the find cursor, not the selection:
        // the selection is what Esc goes back to and must not move under a
        // search that may be abandoned.
        let marked = match finding {
            true => self.find_cursor_pos(&visible).map(|p| visible[p]),
            false => Some(self.selected),
        };

        // A detail line per agent needs two rows each, plus header and footer.
        let detail_lines = rows >= 4 + visible.len() * 2 && width >= 60;
        let show_cwd = width >= 50;

        // Only the first row of a run gets a heading, so a group of six costs
        // one header row rather than six. Suppressed while finding: match order
        // is score order, and a heading over a re-sorted subset would lie.
        let mut group_of: BTreeMap<usize, String> = BTreeMap::new();
        if self.grouping != Grouping::Urgency && !finding {
            let mut prev: Option<&str> = None;
            for (i, a) in self.agents.iter().enumerate() {
                let k = a.group_key(self.grouping);
                if prev != Some(k) {
                    group_of.insert(i, k.to_string());
                    prev = Some(k);
                }
            }
        }

        // One group per agent: its row plus whatever renders underneath it. The
        // viewport pages by group so a selected agent's prompt box is never cut
        // in half.
        let groups: Vec<Vec<Text>> = visible
            .iter()
            .map(|&i| {
                let agent = &self.agents[i];
                let mut g = Vec::new();
                // The heading rides on its first member's group, so the
                // viewport keeps paging by group and never orphans a header.
                if let Some(key) = group_of.get(&i) {
                    let n = self
                        .agents
                        .iter()
                        .filter(|a| a.group_key(self.grouping) == *key)
                        .count();
                    g.push(group_header(key, n, width));
                }
                // Rows keep their real list position, so the number shown is
                // the one `1`-`9` or `g` would take once the prompt closes.
                g.push(agent.list_item(
                    i,
                    crate::agent::RowCtx {
                        selected: Some(i) == marked,
                        icon: self.icon_for(agent),
                        now: self.now,
                        cols: width,
                        show_cwd,
                        home: &self.session_name,
                    },
                ));
                if detail_lines {
                    g.push(agent.detail_item(self.kill_armed.as_ref() == Some(&agent.id), width));
                }
                // The prompt belongs to one agent, so it renders under that
                // row. Not while finding: ask and reply act on the selection,
                // which the find cursor is not.
                if !finding && i == self.selected {
                    if let Some(ask) = self.ask_for(&agent.id) {
                        g.extend(ask_rows(ask, width));
                    }
                    if let Some(reply) = self.reply.as_ref().filter(|r| r.id == agent.id) {
                        g.push(reply_row(&reply.text, width));
                    }
                }
                g
            })
            .collect();

        // Everything that is not a list row: header, its rule, the footer rule,
        // the hints, and the error note when there is one.
        let chrome = 4 + usize::from(self.action_error.is_some());
        let budget = rows.saturating_sub(chrome);
        let keep_visible = marked.and_then(|m| visible.iter().position(|&i| i == m)).unwrap_or(0);
        let view = viewport(&groups, self.scroll, keep_visible, budget);
        self.scroll = view.first;

        let mut items = Vec::new();
        if finding && visible.is_empty() {
            items.push(Text::new(truncate("  no matches", width)).color_range(DIM_LEVEL, ..));
        }
        if view.hidden_above > 0 && view.count > 0 {
            items.push(more_row(view.hidden_above, true, width));
        }
        for g in groups.into_iter().skip(view.first).take(view.count) {
            items.extend(g);
        }
        if view.hidden_below > 0 && view.count > 0 {
            items.push(more_row(view.hidden_below, false, width));
        }
        y = self.render_rows(items, y);
        y = self.render_rule(y, width);
        // A cross-session action that failed is the one thing here the panel
        // cannot show any other way: the row is already gone.
        if let Some(msg) = self.action_error.as_deref() {
            y = self.render_notes(Some((msg.to_string(), true)), y, width);
        }
        let selected_has_ask = self
            .agents
            .get(self.selected)
            .is_some_and(|a| self.ask_for(&a.id).is_some());
        // The find query lives in the footer the way a `g` count does: what has
        // been typed, a block cursor, and the keys that end the search.
        if let Some(f) = self.find.as_ref() {
            print_text_with_coordinates(find_prompt_row(&f.query, width), 0, y, None, None);
            return y + 1;
        }
        // A pending count replaces the footer with what has been typed so far:
        // otherwise the digits vanish into a buffer with nothing on screen.
        // Echoed as vim would show it, count first.
        if let Some(buf) = self.jump_buf.as_deref() {
            let line = truncate(&format!("  g{}\u{2588}   \u{21b5} jump   esc cancel", buf), width);
            let count = chars("  g") + chars(buf) + 1;
            print_text_with_coordinates(Text::new(line).color_range(2, 2..count), 0, y, None, None);
            return y + 1;
        }
        let hints = if self.reply.is_some() {
            ribbon::REPLY_EDIT_HINTS
        } else if selected_has_ask {
            ribbon::ASK_HINTS
        } else if self.can_reply_selected() {
            ribbon::REPLY_HINTS
        } else {
            ribbon::LIST_HINTS
        };
        self.render_hints(hints, y, width);
        y + 1
    }

    /// One row per `Text`, each at its own `y`. Returns the next free row.
    ///
    /// Passes width `None` throughout: a sized component is padded out to that
    /// width, and a row filling the pane wraps and eats the line below it.
    fn render_rows(&self, rows: Vec<Text>, y: usize) -> usize {
        let n = rows.len();
        for (i, row) in rows.into_iter().enumerate() {
            print_text_with_coordinates(row, 0, y + i, None, None);
        }
        y + n
    }

    /// Returns the next free `y`.
    fn render_header(&self, subtitle: &str, width: usize) -> usize {
        let text = format!("zj-agent-mob   {}", subtitle);
        let at = "zj-agent-mob   ".len();
        print_text_with_coordinates(Text::new(text).color_range(DIM_LEVEL, at..), 0, 0, None, None);
        self.render_rule(1, width)
    }

    /// Returns the next free `y`.
    fn render_rule(&self, y: usize, width: usize) -> usize {
        let rule = "\u{2500}".repeat(width);
        print_text_with_coordinates(Text::new(rule).color_range(DIM_LEVEL, ..), 0, y, None, None);
        y + 1
    }

    /// Returns the next free `y`.
    fn render_notes(&self, note: Option<(String, bool)>, y: usize, width: usize) -> usize {
        match note {
            Some((msg, is_error)) => {
                let text = Text::new(format!("  {}", truncate(&msg, width.saturating_sub(2))));
                let text = if is_error {
                    text.error_color_range(..)
                } else {
                    text.color_range(DIM_LEVEL, ..)
                };
                print_text_with_coordinates(text, 0, y, None, None);
                y + 1
            }
            None => y,
        }
    }

    fn render_hints(&self, hints: &[ribbon::Hint], y: usize, width: usize) {
        // Overflowing ribbons lose whole segments rather than truncating, so a
        // narrow pane would silently drop a key. Plain text keeps them all.
        if ribbon::ribbon_width(hints) > width {
            let line = truncate(&ribbon::plain_line(hints), width);
            print_text_with_coordinates(Text::new(line).color_range(DIM_LEVEL, ..), 0, y, Some(width), None);
            return;
        }
        // `selected` alternates the background so adjacent chips stay distinct.
        let texts: Vec<Text> = hints
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let t = Text::new(h.text()).color_range(0, h.key_range());
                if i % 2 == 1 {
                    t.selected()
                } else {
                    t
                }
            })
            .collect();
        // Width `None`: the sized variant stretches the FIRST ribbon to fill it
        // and shoves the rest to the far edge.
        print!("{}", serialize_ribbon_line_with_coordinates(&texts, 0, y, None, None));
    }
}

#[cfg(test)]
mod reply_row_tests {
    use crate::util::testing::item_text;

    /// A line that overflows the pane wraps and eats the row below it, which
    /// desyncs every coordinate after it.
    #[test]
    fn the_reply_row_never_exceeds_the_pane_width() {
        let long = "y".repeat(crate::MAX_REPLY_CHARS);
        for cols in [30usize, 40, 60, 80, 110] {
            for text in ["", "yes", long.as_str()] {
                let row = item_text(&super::reply_row(text, cols));
                assert!(
                    row.chars().count() <= cols,
                    "cols={} text_len={} produced {:?}",
                    cols,
                    text.len(),
                    row
                );
            }
        }
    }

    /// The cursor block is what shows where typing lands.
    #[test]
    fn the_reply_row_shows_a_prompt_and_cursor() {
        let row = item_text(&super::reply_row("yes", 60));
        assert!(row.contains("reply:"), "{:?}", row);
        assert!(row.contains("yes"), "{:?}", row);
        assert!(row.ends_with('\u{2588}'), "{:?}", row);
    }
}

#[cfg(test)]
mod viewport_tests {
    use crate::state::{Grouping, State};
    use std::collections::BTreeMap;

    fn state_with(n: usize) -> State {
        let mut s = State {
            permissions_granted: true,
            session_name: "mob".into(),
            live_sessions: vec!["mob".into()],
            ..Default::default()
        };
        for i in 0..n {
            let args: BTreeMap<String, String> = [
                ("pane_id", i.to_string()),
                ("status", "working".to_string()),
                ("task", format!("task {}", i)),
            ]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
            s.handle_status(&args);
        }
        s
    }

    /// The bug this exists for: rows were emitted from y=2 with no upper bound,
    /// so the footer rule and the key ribbon landed outside the pane.
    #[test]
    fn the_list_never_emits_more_rows_than_the_pane_has() {
        for rows in [6usize, 8, 10, 14, 20, 30] {
            for agents in [1usize, 3, 9, 20, 50] {
                let mut s = state_with(agents);
                for sel in [0, agents / 2, agents - 1] {
                    s.selected = sel;
                    let emitted = s.render_list(rows, 100);
                    assert!(
                        emitted <= rows,
                        "rows={} agents={} selected={} emitted {}",
                        rows,
                        agents,
                        sel,
                        emitted
                    );
                }
            }
        }
    }

    #[test]
    fn a_group_header_names_the_group_and_its_size() {
        let h = crate::util::testing::item_text(&super::group_header("alpha", 3, 40));
        assert!(h.contains("alpha"), "{:?}", h);
        assert!(
            h.contains("(3)"),
            "the count is what survives being cut off below: {:?}",
            h
        );
    }

    #[test]
    fn a_group_header_respects_the_pane_width() {
        for cols in [10usize, 20, 40, 80] {
            let h = crate::util::testing::item_text(&super::group_header(&"p".repeat(60), 12, cols));
            assert!(h.chars().count() <= cols, "cols={} produced {:?}", cols, h);
        }
    }

    /// Group headings add rows the viewport did not have to budget for before,
    /// so the clipping contract is re-asserted under every grouping mode.
    #[test]
    fn grouped_lists_never_emit_more_rows_than_the_pane_has() {
        for grouping in [Grouping::Project, Grouping::Session] {
            for rows in [6usize, 8, 10, 14, 20, 30] {
                for agents in [1usize, 3, 9, 20, 50] {
                    let mut s = state_with(agents);
                    s.grouping = grouping;
                    s.sort_agents();
                    for sel in [0, agents / 2, agents - 1] {
                        s.selected = sel;
                        let emitted = s.render_list(rows, 100);
                        assert!(
                            emitted <= rows,
                            "grouping={:?} rows={} agents={} selected={} emitted {}",
                            grouping,
                            rows,
                            agents,
                            sel,
                            emitted
                        );
                    }
                }
            }
        }
    }

    /// Scroll-to-selection: paging by keyboard must keep the cursor on screen.
    /// Asserted against `viewport` directly, which is the only thing that knows
    /// how many whole groups the window holds.
    #[test]
    fn the_selected_row_stays_inside_the_viewport() {
        let groups: Vec<Vec<zellij_tile::prelude::Text>> = (0..40)
            .map(|i| vec![zellij_tile::prelude::Text::new(format!("row {}", i))])
            .collect();
        let mut scroll = 0;
        // Walks the selection down then back up, carrying `scroll` between
        // frames exactly as the real render does.
        for sel in (0..40).chain((0..40).rev()) {
            let v = super::viewport(&groups, scroll, sel, 8);
            scroll = v.first;
            assert!(v.count > 0, "selected {} produced an empty window", sel);
            assert!(
                v.first <= sel && sel < v.first + v.count,
                "selected {} outside [{}, {})",
                sel,
                v.first,
                v.first + v.count
            );
            assert_eq!(
                v.hidden_above + v.count + v.hidden_below,
                40,
                "every row is either shown or counted as hidden"
            );
        }
    }

    /// The same frame rendered twice must not drift.
    #[test]
    fn a_repeat_render_is_stable() {
        let mut s = state_with(30);
        s.selected = 20;
        let first = s.render_list(12, 100);
        let scroll = s.scroll;
        assert_eq!(s.render_list(12, 100), first);
        assert_eq!(s.scroll, scroll);
    }

    /// An action error costs a row, so the budget has to shrink with it.
    #[test]
    fn an_action_error_does_not_push_the_hints_off_the_pane() {
        let mut s = state_with(30);
        s.action_error = Some("kill failed: no such session".into());
        for rows in [6usize, 9, 15] {
            assert!(s.render_list(rows, 100) <= rows, "rows={}", rows);
        }
    }

    /// A hidden row must be advertised rather than silently dropped.
    #[test]
    fn hidden_rows_are_announced() {
        assert!(crate::util::testing::item_text(&super::more_row(7, false, 40)).contains("7 more"));
        assert!(crate::util::testing::item_text(&super::more_row(2, true, 40)).starts_with("  \u{2191}"));
    }

    /// Scrolling far down then selecting row 0 must page back up.
    #[test]
    fn scrolling_back_to_the_top_is_possible() {
        let mut s = state_with(40);
        s.selected = 39;
        s.render_list(10, 100);
        assert!(s.scroll > 0, "selecting the last row must have scrolled");
        s.selected = 0;
        s.render_list(10, 100);
        assert_eq!(s.scroll, 0, "selecting the first row must scroll back to the top");
    }

    /// A pane too short for even one agent drops every row rather than
    /// spilling. The chrome - header, two rules, hints - is the irreducible
    /// floor; nothing above it is emitted.
    #[test]
    fn a_pane_with_no_room_for_any_row_emits_only_chrome() {
        const CHROME: usize = 4;
        let mut s = state_with(10);
        for rows in [0usize, 1, 2, 3, 4, 5] {
            let emitted = s.render_list(rows, 100);
            assert!(emitted <= CHROME.max(rows), "rows={} emitted {}", rows, emitted);
        }
    }

    /// The exact-fit case: with no rows hidden there is no marker to pay for,
    /// so every agent must be shown rather than one being dropped to make room
    /// for a "0 more" nobody needs.
    #[test]
    fn a_list_that_exactly_fills_the_pane_hides_nothing() {
        // 4 rows of chrome plus one row per agent, detail lines off.
        let mut s = state_with(6);
        let emitted = s.render_list(4 + 6, 100);
        assert_eq!(emitted, 4 + 6, "all six rows fit with no marker");
        assert_eq!(s.scroll, 0);
    }

    /// One row too many: the marker is charged, so one fewer agent is shown and
    /// the count it reports must match what was actually dropped.
    #[test]
    fn the_more_marker_reports_the_true_hidden_count() {
        let mut s = state_with(6);
        // Budget 5: four agents plus the marker row.
        let emitted = s.render_list(4 + 5, 100);
        assert_eq!(emitted, 4 + 5);
        let groups = 6;
        let shown = 5 - 1;
        assert_eq!(groups - shown, 2, "two agents hidden behind the marker");
    }

    /// The reason the window pages by group rather than by row: a selected
    /// agent's prompt box is four rows tall, and showing half of it is worse
    /// than scrolling past the agent above it.
    #[test]
    fn a_tall_group_is_shown_whole_or_not_at_all() {
        use zellij_tile::prelude::Text;
        // Heights 1, 1, 5 (a selected row with a prompt box), 1, 1, ...
        let groups: Vec<Vec<Text>> = (0..10)
            .map(|i| {
                let h = if i % 3 == 2 { 5 } else { 1 };
                (0..h).map(|r| Text::new(format!("{}.{}", i, r))).collect()
            })
            .collect();
        let mut scroll = 0;
        for sel in 0..10 {
            for budget in [3usize, 6, 8, 12] {
                let v = super::viewport(&groups, scroll, sel, budget);
                let rows: usize = groups[v.first..v.first + v.count].iter().map(|g| g.len()).sum();
                let markers = usize::from(v.hidden_above > 0) + usize::from(v.hidden_below > 0);
                assert!(
                    rows + markers <= budget || v.count == 0,
                    "sel={} budget={} used {} rows plus {} markers",
                    sel,
                    budget,
                    rows,
                    markers
                );
            }
            scroll = super::viewport(&groups, scroll, sel, 12).first;
        }
    }

    /// A pending count has to be visible: the digits go into a buffer, and a
    /// footer that still reads "g goto" hides that anything is happening.
    #[test]
    fn a_pending_count_replaces_the_footer_and_still_fits() {
        let mut s = state_with(12);
        s.jump_buf = Some("12".into());
        for rows in [6usize, 10, 20] {
            assert!(s.render_list(rows, 100) <= rows, "rows={}", rows);
        }
    }
}

#[cfg(test)]
mod find_render_tests {
    use crate::state::{Find, State};
    use crate::util::testing::item_text;
    use std::collections::BTreeMap;

    fn state_with(n: usize) -> State {
        let mut s = State {
            permissions_granted: true,
            session_name: "mob".into(),
            live_sessions: vec!["mob".into()],
            ..Default::default()
        };
        for i in 0..n {
            let args: BTreeMap<String, String> = [
                ("pane_id", i.to_string()),
                ("status", "working".to_string()),
                ("task", format!("task {}", i)),
            ]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
            s.handle_status(&args);
        }
        s
    }

    fn finding(n: usize, query: &str) -> State {
        let mut s = state_with(n);
        s.find = Some(Find {
            query: query.into(),
            cursor: None,
        });
        s
    }

    /// The clipping contract, re-asserted with the prompt open: a filtered
    /// list, a "no matches" row, or a full list under an empty query must all
    /// stay inside the pane.
    #[test]
    fn a_filtered_list_never_emits_more_rows_than_the_pane_has() {
        for query in ["", "task 1", "zzz-no-match"] {
            for rows in [6usize, 8, 10, 14, 20, 30] {
                for agents in [1usize, 3, 9, 20, 50] {
                    let mut s = finding(agents, query);
                    let emitted = s.render_list(rows, 100);
                    assert!(
                        emitted <= rows,
                        "query={:?} rows={} agents={} emitted {}",
                        query,
                        rows,
                        agents,
                        emitted
                    );
                }
            }
        }
    }

    #[test]
    fn the_find_prompt_shows_the_query_and_fits_the_pane() {
        for cols in [10usize, 20, 40, 60, 100] {
            let row = item_text(&super::find_prompt_row("alpha", cols));
            assert!(row.chars().count() <= cols, "cols={} produced {:?}", cols, row);
        }
        let row = item_text(&super::find_prompt_row("alpha", 60));
        assert!(row.contains("/alpha\u{2588}"), "{:?}", row);
        assert!(row.contains("esc"), "{:?}", row);
    }

    /// The numbers on screen must be the ones `1`-`9` and `g` act on after the
    /// prompt closes, so a filtered row keeps its real list position.
    #[test]
    fn filtered_rows_keep_their_real_numbers() {
        let s = finding(5, "task 3");
        let matches = s.find_matches();
        assert_eq!(matches.len(), 1);
        let real = matches[0];
        let row = item_text(&s.agents[real].list_item(
            real,
            crate::agent::RowCtx {
                selected: true,
                icon: "\u{25cf}",
                now: 0.0,
                cols: 100,
                show_cwd: false,
                home: "mob",
            },
        ));
        assert!(row.contains(&format!("{} ", real + 1)), "{:?}", row);
    }
}
