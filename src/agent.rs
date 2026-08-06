//! A monitored agent and its panel row.

use zellij_tile::prelude::Text;

use crate::status::Status;
use crate::style::DIM_LEVEL;
use crate::util::{fmt_elapsed, truncate};

pub(crate) struct Agent {
    pub(crate) pane_id: u32,
    pub(crate) tool: String,
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
    /// Falls back to the pane title when there is no transcript summary.
    pub(crate) fn display_task(&self) -> &str {
        match self.task.as_deref() {
            Some(t) if !t.is_empty() => t,
            _ => &self.pane_title,
        }
    }

    /// One agent's row. The icon and status label are themed; the rest is plain.
    pub(crate) fn list_item(
        &self,
        i: usize,
        selected: bool,
        icon: &str,
        now: f64,
        cols: usize,
        show_cwd: bool,
    ) -> Text {
        let marker = if selected { "\u{25b6}" } else { " " };
        let label = self.status.label();

        // Ranges are tracked as the string is built. `Text::serialize` encodes
        // via `as_bytes()`, so these must be byte offsets, and both the marker
        // and the spinner icon are multi-byte.
        let mut text = format!("{} {} ", marker, i + 1);
        let icon_range = text.len()..text.len() + icon.len();
        text.push_str(icon);
        text.push(' ');
        text.push_str(&format!("{:<7} ", self.tool));
        let label_range = text.len()..text.len() + label.len();
        text.push_str(&format!("{:<7} {:>6}", label, fmt_elapsed(now - self.status_since)));

        if show_cwd {
            text.push_str(&format!("  {:<10}", truncate(self.project(), 10)));
        }
        let room = cols.saturating_sub(text.chars().count() + 2);
        if room > 6 {
            text.push_str("  ");
            text.push_str(&truncate(self.display_task(), room));
        }

        let level = self.status.color_level();
        let text = Text::new(text)
            .color_range(level, icon_range)
            .color_range(level, label_range);
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
}

#[cfg(test)]
mod render_tests {
    use super::*;
    use crate::util::testing::{is_selected, item_text};

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

    fn row(a: &Agent, i: usize, selected: bool, icon: &str, now: f64, cols: usize, cwd: bool) -> String {
        item_text(&a.list_item(i, selected, icon, now, cols, cwd))
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
        let sel = a.list_item(0, true, "\u{25cf}", 0.0, 110, true);
        let unsel = a.list_item(0, false, "\u{25cf}", 0.0, 110, true);
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

    /// Drifting byte offsets colour the wrong characters, which the text
    /// assertions above cannot catch.
    #[test]
    fn colour_ranges_land_on_the_icon_and_status_label() {
        for (i, selected, icon) in [(0usize, true, "\u{280b}"), (9, false, "\u{25cf}"), (2, true, "?")] {
            let a = agent();
            let item = a.list_item(i, selected, icon, 0.0, 110, true);
            let text = item_text(&item);
            let marker = if selected { "\u{25b6}" } else { " " };

            let icon_at = marker.len() + 1 + (i + 1).to_string().len() + 1;
            assert_eq!(&text[icon_at..icon_at + icon.len()], icon, "icon offset in {:?}", text);

            let label = a.status.label();
            let label_at = icon_at + icon.len() + 1 + a.tool.len().max(7) + 1;
            assert_eq!(
                &text[label_at..label_at + label.len()],
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

    /// An embedded newline would desync every coordinate below the row.
    #[test]
    fn rows_contain_no_embedded_newlines() {
        let a = agent();
        assert!(!row(&a, 0, true, "\u{280b}", 10.0, 110, true).contains('\n'));
        assert!(!item_text(&a.detail_item(false, 110)).contains('\n'));
    }
}
