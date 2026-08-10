//! Zellij lifecycle and rendering.

use std::collections::BTreeMap;
use zellij_tile::prelude::*;

use crate::state::State;
use crate::status::Status;
use crate::style::{chars, DIM_LEVEL};
use crate::util::truncate;
use crate::{content_width, host, ribbon, PANE_TITLE, TICK};

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
                false
            }
            Event::Timer(_) => {
                self.timer_running = false;
                self.frame = self.frame.wrapping_add(1);
                self.now += TICK;
                let aged = self.age_foreign_rows();
                self.notifier.flush(self.now);
                self.publish_summary();
                // A pending flush keeps the clock running on its own: the window
                // must close even when no row is animating.
                if self.notifier.flush_at.is_some() {
                    self.force_timer();
                } else {
                    self.arm_timer();
                }
                aged || self.agents.iter().any(|a| a.status.is_active())
            }
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
            Text::new("  Press i to check and install the hooks.").color_range(DIM_LEVEL, ..),
        ];
        self.render_rows(rows, y);
    }

    fn render_list(&self, rows: usize, width: usize) {
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
        let head = ranges.into_iter().fold(Text::new(head), |t, (level, is_err, r)| {
            if is_err {
                t.error_color_range(r)
            } else {
                t.color_range(level, r)
            }
        });
        print_text_with_coordinates(head, 0, 0, None, None);
        let mut y = self.render_rule(1, width);

        // A detail line per agent needs two rows each, plus header and footer.
        let detail_lines = rows >= 4 + self.agents.len() * 2 && width >= 60;
        let show_cwd = width >= 50;

        let mut items = Vec::new();
        for (i, agent) in self.agents.iter().enumerate() {
            let icon = self.icon_for(agent);
            items.push(agent.list_item(
                i,
                crate::agent::RowCtx {
                    selected: i == self.selected,
                    icon,
                    now: self.now,
                    cols: width,
                    show_cwd,
                    home: &self.session_name,
                },
            ));
            if detail_lines {
                items.push(agent.detail_item(self.kill_armed.as_ref() == Some(&agent.id), width));
            }
            // The prompt belongs to one agent, so it renders under that row.
            if i == self.selected {
                if let Some(ask) = self.ask_for(&agent.id) {
                    items.extend(ask_rows(ask, width));
                }
                if let Some(reply) = self.reply.as_ref().filter(|r| r.id == agent.id) {
                    items.push(reply_row(&reply.text, width));
                }
            }
        }
        y = self.render_rows(items, y);
        y = self.render_rule(y, width);
        let selected_has_ask = self
            .agents
            .get(self.selected)
            .is_some_and(|a| self.ask_for(&a.id).is_some());
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
