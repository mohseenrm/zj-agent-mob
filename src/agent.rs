//! A monitored agent and its panel row.

use zellij_tile::prelude::Text;

use crate::status::Status;
use crate::style::{chars, DIM_LEVEL};
use crate::util::{fmt_elapsed, truncate};

/// Pane ids are only unique within a session, so the session name is part of
/// an agent's identity.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub(crate) struct AgentId {
    pub(crate) session: String,
    pub(crate) pane_id: u32,
}

/// Mirrors the hook's `LC_ALL=C tr -c 'a-zA-Z0-9._-' '_'`. Folds bytes, not
/// chars, because `tr` does: "café" -> "caf__", and a char-wise fold would look
/// for a spool file the hook never wrote.
///
/// The fold is lossy, so distinct sessions can land on one key: "my session"
/// and "my_session" both give `my_session`, and one spool file would then hold
/// two agents, each write erasing the other. A name the fold altered therefore
/// carries a suffix of its own bytes in hex, which the hook appends the same
/// way. Names the fold leaves alone keep their plain key.
pub(crate) fn sanitize_session(name: &str) -> String {
    let bytes: Vec<u8> = name
        .bytes()
        .map(|b| match b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-') {
            true => b,
            false => b'_',
        })
        .collect();
    let folded = String::from_utf8(bytes).unwrap_or_default();
    if name.is_empty() || folded == name {
        return folded;
    }
    let mut hex = String::new();
    for b in name.bytes().take(8) {
        hex.push_str(&format!("{:02x}", b));
    }
    format!("{}-{}", folded, hex)
}

/// The family and size out of a model id, which is the part that distinguishes
/// two agents in a fleet: `claude-sonnet-4-5-20250929` is `sonnet-4-5`. An id
/// that does not parse is truncated rather than dropped, since an unrecognized
/// model is exactly when the full name is worth seeing.
pub(crate) fn short_model(id: &str) -> String {
    let families = ["opus", "sonnet", "haiku", "fable", "mythos", "gpt", "o1", "o3"];
    let parts: Vec<&str> = id.split('-').collect();
    if let Some(at) = parts
        .iter()
        .position(|p| families.contains(&p.to_ascii_lowercase().as_str()))
    {
        let tail: Vec<&str> = parts[at..]
            .iter()
            .take_while(|p| p.len() < 8 || !p.chars().all(|c| c.is_ascii_digit()))
            .copied()
            .collect();
        return tail.join("-");
    }
    truncate(id, 16)
}

/// Everything a row needs that is not the agent itself. `home` is the panel's
/// own session: a row from anywhere else shows where it lives, since a bare
/// pane number is ambiguous across sessions.
#[derive(Clone, Copy)]
pub(crate) struct RowCtx<'a> {
    pub(crate) selected: bool,
    pub(crate) icon: &'a str,
    pub(crate) now: f64,
    pub(crate) cols: usize,
    pub(crate) show_cwd: bool,
    pub(crate) home: &'a str,
}

pub(crate) struct Agent {
    pub(crate) id: AgentId,
    pub(crate) tool: String,
    /// The agent's own id. Distinguishes a recycled pane from the agent that
    /// used to occupy it, which pane ids alone cannot.
    pub(crate) session_id: String,
    pub(crate) status: Status,
    pub(crate) cwd: String,
    pub(crate) task: Option<String>,
    pub(crate) detail: Option<String>,
    pub(crate) turns: u32,
    pub(crate) status_since: f64,
    /// When a pipe last said anything about this row, change or not.
    pub(crate) last_report: f64,
    /// Timestamp of the newest spool record applied, in the spool's own epoch.
    /// Not comparable with `last_report`, which is on the panel's tick clock.
    pub(crate) spool_ts: f64,
    pub(crate) tab: Option<usize>,
    pub(crate) pane_title: String,
    pub(crate) alive: bool,
    /// Non-default permission modes only; `default` is left empty.
    pub(crate) perm_mode: String,
    /// The model the agent is running, when the hook reported one.
    pub(crate) model: String,
    /// Subagents currently running, and the distinct types seen this turn.
    pub(crate) subagents: u32,
    pub(crate) subagent_types: Vec<String>,
    pub(crate) tasks_total: u32,
    pub(crate) tasks_done: u32,
    /// False once the agent's session stops being listed by Zellij.
    pub(crate) session_alive: bool,
    /// Fired a notification since the panel was last focused.
    pub(crate) notified: bool,
    /// A follow-up is queued for delivery when this turn ends.
    pub(crate) followup_queued: bool,
    /// What kind of answer a blocked agent is waiting for, when it is blocked.
    pub(crate) block: Option<Block>,
}

/// Why an agent is blocked. `detail` already says what the prompt is *about*;
/// this says what kind of answer it wants, which is what decides whether the
/// panel can settle it or you have to read the pane.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum Block {
    /// A tool-permission prompt: approvable from the panel.
    Tool,
    /// A plan waiting to be accepted. Needs reading, so the panel cannot
    /// meaningfully approve it for you.
    Plan,
    /// A free-text question. Answerable, but only with real words.
    Question,
    /// Nobody has typed anything in a while. Not blocked on a decision.
    Idle,
}

impl Block {
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s {
            "tool" => Some(Block::Tool),
            "plan" => Some(Block::Plan),
            "question" => Some(Block::Question),
            "idle" => Some(Block::Idle),
            _ => None,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Block::Tool => "permission",
            Block::Plan => "plan",
            Block::Question => "question",
            Block::Idle => "idle",
        }
    }
}

impl Agent {
    pub(crate) fn pane_id(&self) -> u32 {
        self.id.pane_id
    }

    pub(crate) fn session(&self) -> &str {
        &self.id.session
    }

    /// An agent that has been blocked on you for a long time. The sort already
    /// puts it on top; this says it has stopped being a state and become a fire.
    pub(crate) fn escalated(&self, now: f64) -> bool {
        self.is_blocked() && now - self.status_since >= crate::WAITING_ESCALATE_AFTER
    }

    /// Blocked, but not yet long enough to escalate. The clock has to keep
    /// running for these: a blocked row animates nothing, so nothing else would
    /// advance `now` and the threshold would never be crossed.
    pub(crate) fn escalation_pending(&self, now: f64) -> bool {
        self.is_blocked() && !self.escalated(now)
    }

    fn is_blocked(&self) -> bool {
        matches!(self.status, Status::Waiting | Status::IdleWait)
    }

    /// Falls back to the pane title when there is no transcript summary.
    pub(crate) fn display_task(&self) -> &str {
        match self.task.as_deref() {
            Some(t) if !t.is_empty() => t,
            _ => &self.pane_title,
        }
    }

    /// One agent's row. The icon and status label are themed; the rest is plain.
    pub(crate) fn list_item(&self, i: usize, ctx: RowCtx) -> Text {
        let RowCtx {
            selected,
            icon,
            now,
            cols,
            show_cwd,
            home,
        } = ctx;
        let marker = if selected { "\u{25b6}" } else { " " };
        // Marks a row that notified since you last looked, so coming back from
        // a banner does not mean re-scanning the whole list.
        let bell = if self.notified { "!" } else { " " };
        // `unknown` covers two situations the user acts on differently: the
        // session is gone and the agent is unreachable, or the row simply aged
        // out while its session is alive, which points at missing hooks there.
        let label = match self.status {
            Status::Unknown if !self.session_alive => "gone",
            _ => self.status.label(),
        };

        // Ranges are tracked as the string is built, in CHARACTER offsets: both
        // the marker and the spinner icon are multi-byte, so byte offsets would
        // shift the colour past the icon and into the middle of the next word.
        let mut text = format!("{}{}{:>2} ", marker, bell, i + 1);
        let icon_range = chars(&text)..chars(&text) + chars(icon);
        text.push_str(icon);
        text.push(' ');
        text.push_str(&format!("{:<7} ", self.tool));
        let label_range = chars(&text)..chars(&text) + chars(label);
        // A discovered agent has never reported, so there is no moment to
        // measure from; `0s` would claim it just changed state.
        let elapsed = match self.status {
            Status::Discovered => "--".to_string(),
            _ => fmt_elapsed(now - self.status_since),
        };
        text.push_str(&format!("{:<9} {:>6}", label, elapsed));

        let foreign = !home.is_empty() && self.session() != home;
        let mut session_range = None;
        if show_cwd {
            let col = match foreign {
                true => self.session(),
                false => self.project(),
            };
            text.push_str("  ");
            let start = chars(&text);
            let cell = format!("{:<10}", truncate(col, 10));
            if foreign {
                session_range = Some(start..start + chars(cell.trim_end()));
            }
            text.push_str(&cell);
        }
        // Only rendered when the mode is risky enough to differ from `default`,
        // and only when it fits: a narrow pane drops it like the other columns.
        let badge = match self.perm_mode.is_empty() {
            true => String::new(),
            false => format!("[{}]", truncate(&self.perm_mode, 12)),
        };
        let mode_range = if !badge.is_empty() && text.chars().count() + 2 + chars(&badge) <= cols {
            text.push_str("  ");
            let start = chars(&text);
            text.push_str(&badge);
            Some(start..start + chars(&badge))
        } else {
            None
        };
        let room = cols.saturating_sub(text.chars().count() + 2);
        if room > 6 {
            text.push_str("  ");
            text.push_str(&truncate(self.display_task(), room));
        }

        let level = self.status.color_level();
        let mut text = Text::new(text);
        if self.status.is_error() || self.escalated(now) {
            text = text.error_color_range(icon_range).error_color_range(label_range);
        } else {
            text = text.color_range(level, icon_range).color_range(level, label_range);
        }
        if let Some(r) = mode_range {
            text = text.color_range(2, r);
        }
        if let Some(r) = session_range {
            text = text.color_range(DIM_LEVEL, r);
        }
        if selected {
            text.selected()
        } else {
            text
        }
    }

    /// The dimmed second line under an agent's row.
    pub(crate) fn detail_item(&self, kill_armed: bool, cols: usize) -> Text {
        let mut bits: Vec<String> = Vec::new();
        if kill_armed {
            bits.push("press x again to close pane".to_string());
        } else if self.status == Status::Discovered {
            bits.push("no report yet".to_string());
        } else if let Some(d) = self.detail.as_deref().filter(|d| !d.is_empty()) {
            bits.push(d.to_string());
        }
        // What kind of answer the prompt wants, which decides whether `a`/`r`
        // can settle it here or you have to read the pane. Suppressed when the
        // detail already opens with the word, so it is not said twice.
        if let Some(b) = self.block {
            // The hook's own detail wording, not just the label: a tool prompt
            // arrives as "needs approval: ..." and "permission" never appears.
            // Matched against the hook's own prefix, not any substring: a plan
            // approval reads "needs approval: ExitPlanMode", where a loose
            // search for "plan" hits the tool name and hides the one label that
            // distinguishes it from an ordinary permission.
            let already = |d: &str| match b {
                Block::Tool => d.starts_with("needs approval:"),
                Block::Plan | Block::Question | Block::Idle => false,
            };
            if !self.detail.as_deref().is_some_and(already) {
                bits.insert(0, format!("wants: {}", b.label()));
            }
        }
        if self.subagents > 0 {
            bits.push(match self.subagent_types.is_empty() {
                true => format!("{} subagents", self.subagents),
                false => format!("{} subagents: {}", self.subagents, self.subagent_types.join(", ")),
            });
        }
        // Native task counts are a real progress signal; turns are a proxy.
        if self.tasks_total > 0 {
            bits.push(format!("{}/{} tasks", self.tasks_done, self.tasks_total));
        } else if self.turns > 0 {
            bits.push(format!("{} turns", self.turns));
        }
        if !self.model.is_empty() {
            bits.push(short_model(&self.model));
        }
        if let Some(t) = self.tab {
            bits.push(format!("tab:{}", t + 1));
        }
        bits.push(format!("pane:{}", self.pane_id()));
        if !self.session_alive {
            bits.push("(session exited)".to_string());
        } else if !self.alive {
            bits.push("(pane gone)".to_string());
        }
        // Indented under the row it belongs to, matching the documented layout.
        let text = truncate(&format!("      \u{2514} {}", bits.join(" \u{b7} ")), cols);
        let text = Text::new(text);
        // A pending kill is the one thing here that must not read as chrome.
        if kill_armed {
            text.error_color_range(..)
        } else {
            text.color_range(DIM_LEVEL, ..)
        }
    }

    pub(crate) fn project(&self) -> &str {
        self.cwd.trim_end_matches('/').rsplit('/').next().unwrap_or(&self.cwd)
    }

    /// The heading a row sorts under. An agent that has never reported a `cwd`
    /// has no project to group by, so it gets one bucket rather than an empty
    /// heading that reads as a rendering fault.
    pub(crate) fn group_key(&self, grouping: crate::state::Grouping) -> &str {
        match grouping {
            crate::state::Grouping::Session => self.session(),
            _ => match self.project() {
                "" => "(no cwd)",
                p => p,
            },
        }
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;
    use crate::util::testing::{is_selected, item_text};

    pub(super) fn agent() -> Agent {
        Agent {
            id: AgentId {
                session: "mob".into(),
                pane_id: 3,
            },
            tool: "claude".into(),
            session_id: "s".into(),
            status: Status::Working,
            cwd: "/Users/x/Projects/api".into(),
            task: Some("Add retry to webhook client".into()),
            model: String::new(),
            followup_queued: false,
            detail: Some("Edit src/webhook.rs".into()),
            turns: 4,
            status_since: 0.0,
            last_report: 0.0,
            spool_ts: 0.0,
            tab: Some(1),
            pane_title: "claude".into(),
            alive: true,
            perm_mode: String::new(),
            subagents: 0,
            subagent_types: Vec::new(),
            tasks_total: 0,
            tasks_done: 0,
            session_alive: true,
            notified: false,
            block: None,
        }
    }

    fn ctx<'a>(selected: bool, icon: &'a str, now: f64, cols: usize, show_cwd: bool, home: &'a str) -> RowCtx<'a> {
        RowCtx {
            selected,
            icon,
            now,
            cols,
            show_cwd,
            home,
        }
    }

    fn row(a: &Agent, i: usize, selected: bool, icon: &str, now: f64, cols: usize, cwd: bool) -> String {
        item_text(&a.list_item(i, ctx(selected, icon, now, cols, cwd, "mob")))
    }

    #[test]
    fn row_contains_all_columns() {
        let r = row(&agent(), 0, true, "\u{280b}", 134.0, 110, true);
        for expect in [
            "1",
            "\u{280b}",
            "claude",
            "working",
            "2m14s",
            "api",
            "Add retry to webhook client",
        ] {
            assert!(r.contains(expect), "row {:?} missing {:?}", r, expect);
        }
    }

    /// The selected row gets both the cursor glyph and the `x` highlight flag.
    #[test]
    fn selected_row_is_marked_and_flagged() {
        let a = agent();
        let sel = a.list_item(0, ctx(true, "\u{25cf}", 0.0, 110, true, "mob"));
        let unsel = a.list_item(0, ctx(false, "\u{25cf}", 0.0, 110, true, "mob"));
        assert!(is_selected(&sel));
        assert!(!is_selected(&unsel));
        assert!(
            item_text(&sel).starts_with('\u{25b6}'),
            "selected row leads with the cursor"
        );
        assert!(item_text(&unsel).starts_with(' '), "unselected row is blank there");
    }

    #[test]
    fn row_never_exceeds_cols() {
        let mut a = agent();
        a.task = Some("A very long task summary that must be truncated to fit".into());
        for cols in [40usize, 50, 60, 80, 110] {
            let r = row(&a, 0, true, "\u{25cf}", 0.0, cols, cols >= 50);
            assert!(r.chars().count() <= cols, "cols={} produced {:?}", cols, r);
        }
    }

    #[test]
    fn narrow_panes_drop_cwd_and_task() {
        let a = agent();
        assert!(!row(&a, 0, false, "\u{25cf}", 0.0, 40, false).contains("api"));
        assert!(row(&a, 0, false, "\u{25cf}", 0.0, 110, true).contains("api"));
    }

    /// Drifting offsets colour the wrong characters, which the text assertions
    /// above cannot catch. Zellij indexes colour ranges by character, so these
    /// mirror that: byte offsets here would slide past the multi-byte marker
    /// and icon and highlight only part of the tool name and status label.
    #[test]
    fn colour_ranges_land_on_the_icon_and_status_label() {
        fn slice(text: &str, r: std::ops::Range<usize>) -> String {
            text.chars().skip(r.start).take(r.end - r.start).collect()
        }

        for (i, selected, icon) in [(0usize, true, "\u{280b}"), (9, false, "\u{25cf}"), (2, true, "?")] {
            let a = agent();
            let item = a.list_item(i, ctx(selected, icon, 0.0, 110, true, "mob"));
            let text = item_text(&item);
            let marker = if selected { "\u{25b6}" } else { " " };

            let icon_at = chars(marker) + 1 + 2 + 1;
            assert_eq!(
                slice(&text, icon_at..icon_at + chars(icon)),
                icon,
                "icon offset in {:?}",
                text
            );

            let label = a.status.label();
            let label_at = icon_at + chars(icon) + 1 + a.tool.chars().count().max(7) + 1;
            assert_eq!(
                slice(&text, label_at..label_at + chars(label)),
                label,
                "label offset in {:?}",
                text
            );
        }
    }

    #[test]
    fn detail_line_lists_activity_turns_tab_and_pane() {
        let d = item_text(&agent().detail_item(false, 110));
        assert!(d.contains("Edit src/webhook.rs"));
        assert!(d.contains("4 turns"));
        assert!(
            d.contains("tab:2"),
            "tab is 0-indexed internally, shown 1-based: {:?}",
            d
        );
        assert!(d.contains("pane:3"));
    }

    #[test]
    fn kill_armed_replaces_activity_with_confirmation() {
        let d = item_text(&agent().detail_item(true, 110));
        assert!(d.contains("press x again"), "{:?}", d);
        assert!(!d.contains("Edit src/webhook.rs"));
    }

    #[test]
    fn perm_mode_badge_shows_only_when_set() {
        let mut a = agent();
        assert!(!row(&a, 0, false, "\u{25cf}", 0.0, 110, true).contains('['));
        a.perm_mode = "bypassPermissions".into();
        assert!(row(&a, 0, false, "\u{25cf}", 0.0, 110, true).contains("[bypassPermi…]"));
    }

    /// The badge is an extra column, so the width contract must still hold.
    #[test]
    fn row_with_badge_never_exceeds_cols() {
        let mut a = agent();
        a.perm_mode = "bypassPermissions".into();
        a.task = Some("A very long task summary that must be truncated to fit".into());
        for cols in [40usize, 50, 60, 80, 110] {
            let r = row(&a, 0, true, "\u{25cf}", 0.0, cols, cols >= 50);
            assert!(r.chars().count() <= cols, "cols={} produced {:?}", cols, r);
        }
    }

    #[test]
    fn detail_line_lists_subagents_with_types() {
        let mut a = agent();
        a.subagents = 2;
        a.subagent_types = vec!["Explore".into(), "Plan".into()];
        let d = item_text(&a.detail_item(false, 110));
        assert!(d.contains("2 subagents: Explore, Plan"), "{:?}", d);
    }

    #[test]
    fn task_progress_replaces_turns() {
        let mut a = agent();
        a.tasks_total = 7;
        a.tasks_done = 4;
        let d = item_text(&a.detail_item(false, 110));
        assert!(d.contains("4/7 tasks"), "{:?}", d);
        assert!(!d.contains("4 turns"), "native counts win over the proxy: {:?}", d);
    }

    /// `failed` must not be paintable with a normal theme slot.
    #[test]
    fn failed_status_renders_as_error() {
        let mut a = agent();
        a.status = Status::Failed;
        let r = row(&a, 0, false, "\u{2717}", 0.0, 110, true);
        assert!(r.contains("failed"), "{:?}", r);
        assert!(Status::Failed.is_error());
        assert!(!Status::Working.is_error());
    }

    /// The label column widened to fit `idle-wait`; a narrower pad would shove
    /// the elapsed column left on that one status only.
    #[test]
    fn long_status_label_does_not_shift_columns() {
        let mut a = agent();
        a.status = Status::IdleWait;
        let wide = row(&a, 0, false, "\u{25d0}", 0.0, 110, true);
        a.status = Status::Done;
        let short = row(&a, 0, false, "\u{2713}", 0.0, 110, true);
        let at = |s: &str| s.find("dotfiles").or_else(|| s.find("api"));
        assert_eq!(at(&wide), at(&short), "project column must not move");
    }

    /// Inventing `0s` would claim the agent just changed state; the panel does
    /// not know when a discovered agent last did anything.
    #[test]
    fn discovered_row_shows_no_elapsed_time() {
        let mut a = agent();
        a.status = Status::Discovered;
        a.task = None;
        a.pane_title = String::new();
        let r = row(&a, 0, false, "\u{25cc}", 134.0, 110, true);
        assert!(r.contains("found"), "{:?}", r);
        assert!(r.contains("--"), "{:?}", r);
        assert!(!r.contains("2m14s"), "elapsed is unknown, not zero: {:?}", r);
    }

    /// The row must say why it is bare rather than looking like a broken read.
    #[test]
    fn block_kinds_parse_and_label() {
        assert_eq!(Block::parse("tool"), Some(Block::Tool));
        assert_eq!(Block::parse("plan"), Some(Block::Plan));
        assert_eq!(Block::parse("question"), Some(Block::Question));
        assert_eq!(Block::parse("idle"), Some(Block::Idle));
        assert_eq!(Block::parse("nonsense"), None);
        assert_eq!(Block::Tool.label(), "permission");
    }

    #[test]
    fn the_detail_line_says_what_the_prompt_wants() {
        let mut a = agent();
        a.detail = Some("Bash rm -rf node_modules".into());
        a.block = Some(Block::Plan);
        let d = item_text(&a.detail_item(false, 110));
        assert!(d.contains("wants: plan"), "{:?}", d);
    }

    /// The detail text often already names the kind; saying it twice wastes the
    /// one line there is.
    #[test]
    fn the_wants_label_is_not_repeated_when_the_detail_already_says_it() {
        let mut a = agent();
        a.detail = Some("needs approval: git push".into());
        a.block = Some(Block::Tool);
        let d = item_text(&a.detail_item(false, 110));
        assert_eq!(
            d.matches("permission").count(),
            0,
            "detail said approval already: {:?}",
            d
        );
    }

    /// The regression: a plan approval reads "needs approval: ExitPlanMode", and
    /// a substring search for "plan" hits the tool name and suppressed the one
    /// label that tells a plan apart from an ordinary permission.
    #[test]
    fn a_plan_approval_still_says_it_wants_a_plan() {
        let mut a = agent();
        a.detail = Some("needs approval: ExitPlanMode".into());
        a.block = Some(Block::Plan);
        let d = item_text(&a.detail_item(false, 110));
        assert!(d.contains("wants: plan"), "{:?}", d);
    }

    #[test]
    fn the_detail_line_with_a_block_still_respects_cols() {
        for cols in [30usize, 40, 60, 80, 110] {
            let mut a = agent();
            a.block = Some(Block::Question);
            a.detail = Some("x".repeat(200));
            let d = item_text(&a.detail_item(false, cols));
            assert!(d.chars().count() <= cols, "cols={} produced {:?}", cols, d);
        }
    }

    #[test]
    fn discovered_detail_line_says_no_report_yet() {
        let mut a = agent();
        a.status = Status::Discovered;
        a.detail = None;
        a.turns = 0;
        let d = item_text(&a.detail_item(false, 110));
        assert!(d.contains("no report yet"), "{:?}", d);
        assert!(d.contains("pane:3"), "{:?}", d);
    }

    /// The status column is padded to a fixed width; a new label must not push
    /// the columns after it out of alignment.
    #[test]
    fn found_label_does_not_shift_columns() {
        let mut a = agent();
        a.status = Status::Discovered;
        let disc = row(&a, 0, false, "\u{25cc}", 0.0, 110, true);
        a.status = Status::Done;
        let done = row(&a, 0, false, "\u{2713}", 0.0, 110, true);
        assert_eq!(disc.find("api"), done.find("api"), "project column must not move");
    }

    #[test]
    fn dead_pane_is_flagged() {
        let mut a = agent();
        a.alive = false;
        assert!(item_text(&a.detail_item(false, 110)).contains("(pane gone)"));
    }

    #[test]
    fn detail_line_respects_cols() {
        for cols in [30usize, 60, 110] {
            let d = item_text(&agent().detail_item(false, cols));
            assert!(d.chars().count() <= cols, "cols={} got {:?}", cols, d);
        }
    }

    /// A bare pane number is ambiguous once rows span sessions, so a foreign row
    /// says where it lives instead of showing its project.
    #[test]
    fn a_foreign_row_shows_its_session_instead_of_the_project() {
        let a = agent();
        let home = item_text(&a.list_item(0, ctx(false, "\u{25cf}", 0.0, 110, true, "mob")));
        assert!(home.contains("api"), "own session shows the project: {:?}", home);
        assert!(!home.contains("mob"), "and not its own name: {:?}", home);

        let away = item_text(&a.list_item(0, ctx(false, "\u{25cf}", 0.0, 110, true, "elsewhere")));
        assert!(away.contains("mob"), "foreign row names its session: {:?}", away);
    }

    /// The session replaces the project column rather than adding one, so the
    /// width contract and every column after it must be unmoved.
    #[test]
    fn the_session_column_does_not_shift_later_columns() {
        let mut a = agent();
        a.task = Some("Add retry to webhook client".into());
        let home = item_text(&a.list_item(0, ctx(false, "\u{25cf}", 0.0, 110, true, "mob")));
        let away = item_text(&a.list_item(0, ctx(false, "\u{25cf}", 0.0, 110, true, "elsewhere")));
        assert_eq!(
            home.find("Add retry"),
            away.find("Add retry"),
            "task column must not move"
        );
        for cols in [40usize, 50, 60, 80, 110] {
            let r = item_text(&a.list_item(0, ctx(true, "\u{25cf}", 0.0, cols, cols >= 50, "elsewhere")));
            assert!(r.chars().count() <= cols, "cols={} produced {:?}", cols, r);
        }
    }

    /// With no session known yet every row would otherwise read as foreign.
    #[test]
    fn an_unknown_home_session_leaves_rows_local() {
        let r = item_text(&agent().list_item(0, ctx(false, "\u{25cf}", 0.0, 110, true, "")));
        assert!(r.contains("api"), "{:?}", r);
    }

    #[test]
    fn a_dead_session_row_says_so() {
        let mut a = agent();
        a.session_alive = false;
        a.status = Status::Unknown;
        let row = item_text(&a.list_item(0, ctx(false, "?", 0.0, 110, true, "mob")));
        assert!(row.contains("gone"), "the list row is what you scan: {:?}", row);
        assert!(
            !row.contains("unknown"),
            "a gone session is not merely stale: {:?}",
            row
        );
        let d = item_text(&a.detail_item(false, 110));
        assert!(d.contains("(session exited)"), "{:?}", d);
        assert!(!d.contains("(pane gone)"), "the session is the bigger fact: {:?}", d);
    }

    /// The other half: a row that aged out while its session is still alive is
    /// stale, not gone, and usually means hooks are missing over there.
    #[test]
    fn a_stale_row_in_a_live_session_still_says_unknown() {
        let mut a = agent();
        a.status = Status::Unknown;
        let row = item_text(&a.list_item(0, ctx(false, "?", 0.0, 110, true, "mob")));
        assert!(row.contains("unknown"), "{:?}", row);
        assert!(!row.contains("gone"), "the session is alive: {:?}", row);
    }

    /// The sort already puts a blocked agent on top; past a threshold the colour
    /// says it has stopped being a state and become a fire.
    #[test]
    fn a_long_wait_escalates_to_the_error_colour() {
        let mut a = agent();
        a.status = Status::Waiting;
        a.status_since = 0.0;
        assert!(!a.escalated(5.0), "a prompt you are answering must not escalate");
        assert!(!a.escalated(crate::WAITING_ESCALATE_AFTER - 1.0));
        assert!(a.escalated(crate::WAITING_ESCALATE_AFTER));
    }

    /// Only a blocked agent can escalate: a long-running turn is working as
    /// intended, and painting it as an error would cry wolf.
    #[test]
    fn only_blocked_statuses_escalate() {
        let mut a = agent();
        a.status_since = 0.0;
        for status in [Status::Working, Status::Done, Status::Idle, Status::Discovered] {
            a.status = status;
            assert!(!a.escalated(10_000.0), "{:?} must not escalate", status);
        }
        for status in [Status::Waiting, Status::IdleWait] {
            a.status = status;
            assert!(a.escalated(10_000.0), "{:?} should escalate", status);
        }
    }

    /// An embedded newline would desync every coordinate below the row.
    #[test]
    fn rows_contain_no_embedded_newlines() {
        let a = agent();
        assert!(!row(&a, 0, true, "\u{280b}", 10.0, 110, true).contains('\n'));
        assert!(!item_text(&a.detail_item(false, 110)).contains('\n'));
    }

    /// Coming back from a banner should not mean re-scanning the whole list.
    #[test]
    fn a_notified_row_carries_a_gutter_marker() {
        let mut a = agent();
        assert!(
            !row(&a, 0, false, "\u{25cf}", 0.0, 110, true).starts_with("  1"),
            "unmarked rows are blank there"
        );
        a.notified = true;
        let marked = row(&a, 0, false, "\u{25cf}", 0.0, 110, true);
        assert!(marked.starts_with(" ! 1"), "{:?}", marked);
    }

    /// The marker sits in its own column, so it must not shift the row's other
    /// fields - the colour ranges are computed from those offsets.
    #[test]
    fn the_gutter_marker_does_not_shift_the_columns() {
        let mut a = agent();
        let plain = row(&a, 0, false, "\u{25cf}", 0.0, 110, true);
        a.notified = true;
        let marked = row(&a, 0, false, "\u{25cf}", 0.0, 110, true);
        assert_eq!(plain.chars().count(), marked.chars().count());
        let tail = |s: &str| s.chars().skip(2).collect::<String>();
        assert_eq!(tail(&plain), tail(&marked), "only the gutter column differs");
    }

    /// Rows past 9 are numbered too: `G` reaches them, so the number is not a
    /// promise only the 1-9 keys can keep.
    #[test]
    fn rows_past_nine_keep_their_number() {
        let r = row(&agent(), 24, false, "\u{25cf}", 0.0, 110, true);
        assert!(r.starts_with("  25 "), "{:?}", r);
    }

    /// Pins the exact gutter the docs show, so a layout change cannot silently
    /// make the troubleshooting sample wrong.
    #[test]
    fn the_documented_sample_row_renders_as_documented() {
        let mut a = agent();
        a.status = Status::Discovered;
        a.task = None;
        a.pane_title = String::new();
        let r = row(&a, 0, false, "\u{25cc}", 0.0, 110, false);
        assert!(r.starts_with("   1 \u{25cc} claude  found        "), "{:?}", r);
    }
}

#[cfg(test)]
mod model_tests {
    use super::short_model;

    #[test]
    fn a_model_id_reduces_to_family_and_size() {
        assert_eq!(short_model("claude-sonnet-4-5-20250929"), "sonnet-4-5");
        assert_eq!(short_model("claude-opus-5"), "opus-5");
        assert_eq!(short_model("claude-haiku-4-5-20251001"), "haiku-4-5");
        assert_eq!(short_model("gpt-5-codex"), "gpt-5-codex");
    }

    /// An id that does not parse is exactly when the full name is worth seeing,
    /// so it is truncated rather than dropped.
    #[test]
    fn an_unrecognized_id_survives_rather_than_vanishing() {
        assert_eq!(short_model("some-new-thing"), "some-new-thing");
        assert!(!short_model(&"x".repeat(40)).is_empty());
        assert!(short_model(&"x".repeat(40)).chars().count() <= 16);
    }

    #[test]
    fn the_model_appears_on_the_detail_line() {
        let mut a = super::render_tests::agent();
        a.model = "claude-sonnet-4-5-20250929".into();
        let line = crate::util::testing::item_text(&a.detail_item(false, 200));
        assert!(line.contains("sonnet-4-5"), "{:?}", line);
    }

    #[test]
    fn no_model_adds_nothing_to_the_line() {
        let a = super::render_tests::agent();
        let line = crate::util::testing::item_text(&a.detail_item(false, 200));
        assert!(!line.contains("sonnet"), "{:?}", line);
    }
}
