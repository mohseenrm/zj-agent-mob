//! A single monitored agent, and how one row of the panel is built.
//!
//! `plain_row` / `detail_line` are pure string builders: all width math and
//! truncation lives here so tests can assert exact layout without ANSI noise.
//! `styled_row` layers colour on top without changing widths.

use crate::status::Status;
use crate::style::{BOLD, RESET, SEL_BG};
use crate::util::{fmt_elapsed, truncate};

pub(crate) struct Agent {
    pub(crate) pane_id: u32,
    pub(crate) tool: String,
    /// Kept for debugging and future per-session features (e.g. resume).
    #[allow(dead_code)]
    pub(crate) session_id: String,
    pub(crate) status: Status,
    pub(crate) cwd: String,
    pub(crate) task: Option<String>,
    pub(crate) detail: Option<String>,
    pub(crate) turns: u32,
    pub(crate) status_since: f64,
    pub(crate) tab: Option<usize>,
    pub(crate) pane_title: String,
    pub(crate) alive: bool,
}

impl Agent {
    /// Task summary, falling back to the pane title when no transcript summary exists.
    pub(crate) fn display_task(&self) -> &str {
        match self.task.as_deref() {
            Some(t) if !t.is_empty() => t,
            _ => &self.pane_title,
        }
    }

    /// Plain (unstyled) row text. Width math and truncation live here so tests
    /// can assert exact layout without ANSI noise.
    pub(crate) fn plain_row(
        &self,
        i: usize,
        selected: bool,
        icon: &str,
        now: f64,
        cols: usize,
        show_cwd: bool,
    ) -> String {
        let marker = if selected { "\u{25b6}" } else { " " };
        let elapsed = fmt_elapsed(now - self.status_since);
        let mut plain = format!(
            "{} {} {} {:<7} {:<7} {:>6}",
            marker,
            i + 1,
            icon,
            self.tool,
            self.status.label(),
            elapsed
        );
        if show_cwd {
            plain.push_str(&format!("  {:<10}", truncate(self.project(), 10)));
        }
        let room = cols.saturating_sub(plain.chars().count() + 2);
        if room > 6 {
            plain.push_str("  ");
            plain.push_str(&truncate(self.display_task(), room));
        }
        plain
    }

    /// Same layout as `plain_row`, with colour applied.
    pub(crate) fn styled_row(
        &self,
        i: usize,
        selected: bool,
        icon: &str,
        now: f64,
        cols: usize,
        show_cwd: bool,
    ) -> String {
        let plain = self.plain_row(i, selected, icon, now, cols, show_cwd);
        // Colour the status icon and label in place, keeping the plain widths.
        let colored_icon = format!("{}{}{}{}", self.status.ansi(), BOLD, icon, RESET);
        let label = self.status.label();
        let colored_label = format!("{}{}{}", self.status.ansi(), label, RESET);
        let mut out = plain
            .replacen(icon, &colored_icon, 1)
            .replacen(label, &colored_label, 1);
        if selected {
            out = format!("{}{}{}", SEL_BG, out, RESET);
        }
        out.push_str(RESET);
        out
    }

    pub(crate) fn detail_line(&self, kill_armed: bool, cols: usize) -> String {
        let mut bits: Vec<String> = Vec::new();
        if kill_armed {
            bits.push("press x again to close pane".to_string());
        } else if let Some(d) = self.detail.as_deref().filter(|d| !d.is_empty()) {
            bits.push(d.to_string());
        }
        if self.turns > 0 {
            bits.push(format!("{} turns", self.turns));
        }
        if let Some(t) = self.tab {
            bits.push(format!("tab:{}", t + 1));
        }
        bits.push(format!("pane:{}", self.pane_id));
        if !self.alive {
            bits.push("(pane gone)".to_string());
        }
        truncate(&format!("      \u{2514} {}", bits.join(" \u{b7} ")), cols)
    }

    pub(crate) fn project(&self) -> &str {
        self.cwd
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(&self.cwd)
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;

    fn agent() -> Agent {
        Agent {
            pane_id: 3,
            tool: "claude".into(),
            session_id: "s".into(),
            status: Status::Working,
            cwd: "/Users/x/Projects/api".into(),
            task: Some("Add retry to webhook client".into()),
            detail: Some("Edit src/webhook.rs".into()),
            turns: 4,
            status_since: 0.0,
            tab: Some(1),
            pane_title: "claude".into(),
            alive: true,
        }
    }

    #[test]
    fn row_contains_all_columns() {
        let a = agent();
        let row = a.plain_row(0, true, "\u{280b}", 134.0, 110, true);
        for expect in ["\u{25b6}", "1", "\u{280b}", "claude", "working", "2m14s", "api", "Add retry to webhook client"] {
            assert!(row.contains(expect), "row {:?} missing {:?}", row, expect);
        }
    }

    #[test]
    fn row_never_exceeds_cols() {
        let mut a = agent();
        a.task = Some("A very long task summary that must be truncated to fit".into());
        for cols in [40usize, 50, 60, 80, 110] {
            let row = a.plain_row(0, true, "\u{25cf}", 0.0, cols, cols >= 50);
            assert!(
                row.chars().count() <= cols,
                "cols={} produced {} chars: {:?}",
                cols, row.chars().count(), row
            );
        }
    }

    #[test]
    fn narrow_panes_drop_cwd_and_task() {
        let a = agent();
        let narrow = a.plain_row(0, false, "\u{25cf}", 0.0, 40, false);
        assert!(!narrow.contains("api"), "cwd must be dropped when show_cwd=false");
        let wide = a.plain_row(0, false, "\u{25cf}", 0.0, 110, true);
        assert!(wide.contains("api"));
    }

    #[test]
    fn detail_line_lists_activity_turns_tab_and_pane() {
        let d = agent().detail_line(false, 110);
        assert!(d.contains("Edit src/webhook.rs"));
        assert!(d.contains("4 turns"));
        assert!(d.contains("tab:2"), "tab is 0-indexed internally, displayed 1-based: {:?}", d);
        assert!(d.contains("pane:3"));
    }

    #[test]
    fn kill_armed_replaces_activity_with_confirmation() {
        let d = agent().detail_line(true, 110);
        assert!(d.contains("press x again"), "{:?}", d);
        assert!(!d.contains("Edit src/webhook.rs"));
    }

    #[test]
    fn dead_pane_is_flagged() {
        let mut a = agent();
        a.alive = false;
        assert!(a.detail_line(false, 110).contains("(pane gone)"));
    }

    #[test]
    fn detail_line_respects_cols() {
        for cols in [30usize, 60, 110] {
            let d = agent().detail_line(false, cols);
            assert!(d.chars().count() <= cols, "cols={} got {:?}", cols, d);
        }
    }

    /// The bug that made every row collapse onto one grid line: rows must not
    /// contain embedded newlines, and the caller emits exactly one println per row.
    #[test]
    fn rows_contain_no_embedded_newlines() {
        let a = agent();
        let row = a.styled_row(0, true, "\u{280b}", 10.0, 110, true);
        let detail = a.detail_line(false, 110);
        assert!(!row.contains('\n'), "row must be a single line");
        assert!(!detail.contains('\n'), "detail must be a single line");
    }

    #[test]
    fn styled_row_preserves_plain_text_content() {
        let a = agent();
        let plain = a.plain_row(0, false, "\u{25cf}", 0.0, 110, true);
        let styled = a.styled_row(0, false, "\u{25cf}", 0.0, 110, true);
        // Stripping ANSI from the styled row must recover the plain row.
        let mut stripped = String::new();
        let mut chars = styled.chars();
        while let Some(c) = chars.next() {
            if c == '\u{1b}' {
                for c2 in chars.by_ref() {
                    if c2 == 'm' {
                        break;
                    }
                }
            } else {
                stripped.push(c);
            }
        }
        assert_eq!(stripped, plain);
    }
}
