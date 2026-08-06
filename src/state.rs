//! Panel state: pipe handling, pane reconciliation.

use std::collections::BTreeMap;
use zellij_tile::prelude::*;

use crate::agent::Agent;
use crate::host;
use crate::status::Status;
use crate::{SPINNER, TICK};

#[derive(Default)]
pub struct State {
    pub(crate) agents: Vec<Agent>,
    pub(crate) selected: usize,
    pub(crate) frame: usize,
    pub(crate) now: f64,
    pub(crate) permissions_granted: bool,
    pub(crate) kill_armed: Option<u32>,
    pub(crate) timer_running: bool,
    pub(crate) popup_on_waiting: bool,
    pub(crate) hidden: bool,
}

impl State {
    pub(crate) fn icon_for(&self, agent: &Agent) -> &'static str {
        match agent.status {
            Status::Working => SPINNER[self.frame % SPINNER.len()],
            Status::Waiting => "\u{25cf}",
            Status::Done => "\u{2713}",
            Status::Idle => "\u{25cb}",
        }
    }

    pub(crate) fn counts(&self) -> (usize, usize, usize) {
        let mut c = (0, 0, 0);
        for a in &self.agents {
            match a.status {
                Status::Waiting => c.0 += 1,
                Status::Working => c.1 += 1,
                Status::Done => c.2 += 1,
                Status::Idle => {}
            }
        }
        c
    }

    pub(crate) fn arm_timer(&mut self) {
        if !self.timer_running && self.agents.iter().any(|a| a.status == Status::Working) {
            self.timer_running = true;
            host::set_timeout(TICK);
        }
    }

    pub(crate) fn handle_status(&mut self, args: &BTreeMap<String, String>) -> bool {
        let Some(pane_id) = args.get("pane_id").and_then(|v| v.parse::<u32>().ok()) else {
            return false;
        };
        let raw_status = args.get("status").map(|s| s.as_str()).unwrap_or("");

        if raw_status == "ended" {
            let before = self.agents.len();
            self.agents.retain(|a| a.pane_id != pane_id);
            self.clamp_selection();
            return self.agents.len() != before;
        }

        let Some(status) = Status::parse(raw_status) else {
            return false;
        };

        // Empty means "unchanged": heartbeats omit these to avoid re-reading
        // multi-MB transcripts.
        let task = args.get("task").filter(|t| !t.is_empty()).cloned();
        let detail = args.get("detail").filter(|d| !d.is_empty()).cloned();
        let now = self.now;

        let newly_waiting;
        if let Some(agent) = self.agents.iter_mut().find(|a| a.pane_id == pane_id) {
            let changed = agent.status != status;
            newly_waiting = changed && status == Status::Waiting;
            if changed {
                agent.status_since = now;
                if status == Status::Working {
                    agent.turns += 1;
                }
            }
            agent.status = status;
            if task.is_some() {
                agent.task = task;
            }
            if detail.is_some() {
                agent.detail = detail;
            }
            if let Some(cwd) = args.get("cwd").filter(|c| !c.is_empty()) {
                agent.cwd = cwd.clone();
            }
            agent.alive = true;
        } else {
            newly_waiting = status == Status::Waiting;
            self.agents.push(Agent {
                pane_id,
                tool: args.get("tool").cloned().unwrap_or_else(|| "agent".into()),
                session_id: args.get("session_id").cloned().unwrap_or_default(),
                status,
                cwd: args.get("cwd").cloned().unwrap_or_default(),
                task,
                detail,
                turns: if status == Status::Working { 1 } else { 0 },
                status_since: now,
                tab: None,
                pane_title: String::new(),
                alive: true,
            });
        }

        self.sort_agents();
        self.arm_timer();

        if newly_waiting && self.popup_on_waiting && self.hidden {
            if let Some(idx) = self.agents.iter().position(|a| a.pane_id == pane_id) {
                self.selected = idx;
            }
            self.hidden = false;
            host::show_self(true);
        }
        true
    }

    pub(crate) fn handle_label(&mut self, args: &BTreeMap<String, String>) -> bool {
        let Some(pane_id) = args.get("pane_id").and_then(|v| v.parse::<u32>().ok()) else {
            return false;
        };
        let Some(label) = args.get("label") else {
            return false;
        };
        if let Some(agent) = self.agents.iter_mut().find(|a| a.pane_id == pane_id) {
            agent.task = Some(label.clone());
            return true;
        }
        false
    }

    pub(crate) fn reconcile(&mut self, manifest: PaneManifest) {
        for agent in self.agents.iter_mut() {
            agent.alive = false;
        }
        let mut saw_any_terminal = false;
        for (tab, panes) in manifest.panes {
            for pane in panes {
                // Agents only run in terminal panes.
                if pane.is_plugin {
                    continue;
                }
                saw_any_terminal = true;
                if let Some(agent) = self.agents.iter_mut().find(|a| a.pane_id == pane.id) {
                    agent.alive = true;
                    agent.tab = Some(tab);
                    agent.pane_title = pane.title.clone();
                }
            }
        }
        // Drop agents whose pane is gone, but only once we've actually seen a
        // terminal pane: a pipe can land before the first PaneUpdate.
        if saw_any_terminal {
            self.agents.retain(|a| a.alive);
        }
        self.clamp_selection();
        self.sort_agents();
    }

    pub(crate) fn sort_agents(&mut self) {
        self.agents
            .sort_by(|a, b| a.status.rank().cmp(&b.status.rank()).then(a.pane_id.cmp(&b.pane_id)));
        self.clamp_selection();
    }

    pub(crate) fn clamp_selection(&mut self) {
        if self.agents.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.agents.len() {
            self.selected = self.agents.len() - 1;
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn state() -> State {
        State {
            permissions_granted: true,
            popup_on_waiting: false,
            ..Default::default()
        }
    }

    #[test]
    fn parses_known_statuses_only() {
        assert!(Status::parse("working").is_some());
        assert!(Status::parse("waiting").is_some());
        assert!(Status::parse("done").is_some());
        assert!(Status::parse("idle").is_some());
        assert!(Status::parse("ended").is_none());
        assert!(Status::parse("garbage").is_none());
    }

    #[test]
    fn upsert_creates_then_updates_in_place() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "3"), ("status", "working"), ("tool", "claude")]));
        assert_eq!(s.agents.len(), 1);
        s.handle_status(&args(&[("pane_id", "3"), ("status", "done")]));
        assert_eq!(s.agents.len(), 1, "same pane must not duplicate");
        assert!(s.agents[0].status == Status::Done);
    }

    /// Heartbeats send an empty task; it must not blank an existing summary.
    #[test]
    fn empty_task_does_not_clear_existing_summary() {
        let mut s = state();
        s.handle_status(&args(&[
            ("pane_id", "1"),
            ("status", "working"),
            ("task", "Fix the parser"),
        ]));
        assert_eq!(s.agents[0].task.as_deref(), Some("Fix the parser"));

        s.handle_status(&args(&[("pane_id", "1"), ("status", "working"), ("task", "")]));
        assert_eq!(
            s.agents[0].task.as_deref(),
            Some("Fix the parser"),
            "empty task must mean 'unchanged', not 'clear'"
        );

        s.handle_status(&args(&[("pane_id", "1"), ("status", "done")]));
        assert_eq!(s.agents[0].task.as_deref(), Some("Fix the parser"));

        s.handle_status(&args(&[("pane_id", "1"), ("status", "working"), ("task", "New task")]));
        assert_eq!(s.agents[0].task.as_deref(), Some("New task"));
    }

    #[test]
    fn ended_removes_agent() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "4"), ("status", "idle")]));
        assert_eq!(s.agents.len(), 1);
        assert!(s.handle_status(&args(&[("pane_id", "4"), ("status", "ended")])));
        assert!(s.agents.is_empty());
    }

    #[test]
    fn malformed_pipes_are_ignored() {
        let mut s = state();
        assert!(!s.handle_status(&args(&[("status", "working")])), "no pane_id");
        assert!(!s.handle_status(&args(&[("pane_id", "notanumber"), ("status", "working")])));
        assert!(!s.handle_status(&args(&[("pane_id", "1"), ("status", "bogus")])));
        assert!(s.agents.is_empty());
    }

    #[test]
    fn waiting_sorts_before_working_and_idle() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "idle")]));
        s.handle_status(&args(&[("pane_id", "2"), ("status", "working")]));
        s.handle_status(&args(&[("pane_id", "3"), ("status", "waiting")]));
        s.handle_status(&args(&[("pane_id", "4"), ("status", "done")]));
        let order: Vec<&str> = s.agents.iter().map(|a| a.status.label()).collect();
        assert_eq!(order, vec!["waiting", "done", "working", "idle"]);
    }

    #[test]
    fn status_since_only_resets_on_actual_change() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "working")]));
        s.now = 10.0;
        s.handle_status(&args(&[("pane_id", "1"), ("status", "working")]));
        assert_eq!(s.agents[0].status_since, 0.0, "heartbeat must not reset elapsed");
        s.handle_status(&args(&[("pane_id", "1"), ("status", "done")]));
        assert_eq!(s.agents[0].status_since, 10.0);
    }

    #[test]
    fn turns_increment_only_on_new_working_turns() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "working")]));
        assert_eq!(s.agents[0].turns, 1);
        s.handle_status(&args(&[("pane_id", "1"), ("status", "working")])); // heartbeat
        assert_eq!(s.agents[0].turns, 1, "heartbeats must not inflate turn count");
        s.handle_status(&args(&[("pane_id", "1"), ("status", "done")]));
        s.handle_status(&args(&[("pane_id", "1"), ("status", "working")])); // new turn
        assert_eq!(s.agents[0].turns, 2);
    }

    #[test]
    fn selection_stays_in_bounds_when_agents_disappear() {
        let mut s = state();
        for id in 1..=3 {
            s.handle_status(&args(&[("pane_id", &id.to_string()), ("status", "idle")]));
        }
        s.selected = 2;
        s.handle_status(&args(&[("pane_id", "3"), ("status", "ended")]));
        assert!(s.selected < s.agents.len(), "selection must be clamped");
        s.handle_status(&args(&[("pane_id", "2"), ("status", "ended")]));
        s.handle_status(&args(&[("pane_id", "1"), ("status", "ended")]));
        assert_eq!(s.selected, 0, "empty list selects index 0");
    }

    #[test]
    fn display_task_falls_back_to_pane_title() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "idle")]));
        s.agents[0].pane_title = "nvim src/lib.rs".to_string();
        assert_eq!(s.agents[0].display_task(), "nvim src/lib.rs");
        s.agents[0].task = Some("Real summary".to_string());
        assert_eq!(s.agents[0].display_task(), "Real summary");
    }

    #[test]
    fn project_is_cwd_basename() {
        let mut s = state();
        s.handle_status(&args(&[
            ("pane_id", "1"),
            ("status", "idle"),
            ("cwd", "/Users/x/Projects/api"),
        ]));
        assert_eq!(s.agents[0].project(), "api");
        s.agents[0].cwd = "/Users/x/Projects/web/".to_string();
        assert_eq!(s.agents[0].project(), "web", "trailing slash must be handled");
    }

    #[test]
    fn label_pipe_overrides_task() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "idle"), ("task", "auto")]));
        assert!(s.handle_label(&args(&[("pane_id", "1"), ("label", "manual")])));
        assert_eq!(s.agents[0].task.as_deref(), Some("manual"));
        assert!(!s.handle_label(&args(&[("pane_id", "99"), ("label", "x")])), "unknown pane");
    }



    #[test]
    fn counts_summarize_by_status() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "waiting")]));
        s.handle_status(&args(&[("pane_id", "2"), ("status", "working")]));
        s.handle_status(&args(&[("pane_id", "3"), ("status", "working")]));
        s.handle_status(&args(&[("pane_id", "4"), ("status", "done")]));
        s.handle_status(&args(&[("pane_id", "5"), ("status", "idle")]));
        assert_eq!(s.counts(), (1, 2, 1));
    }
}

#[cfg(test)]
mod reconcile_tests {
    use super::*;

    fn args(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn manifest(panes: &[(usize, u32)]) -> PaneManifest {
        let mut m = PaneManifest { panes: std::collections::HashMap::new() };
        for (tab, id) in panes {
            let p = PaneInfo {
                id: *id,
                is_plugin: false,
                title: format!("pane {}", id),
                ..Default::default()
            };
            m.panes.entry(*tab).or_default().push(p);
        }
        m
    }

    #[test]
    fn agent_on_live_pane_survives_reconcile() {
        let mut s = State { permissions_granted: true, ..Default::default() };
        s.handle_status(&args(&[("pane_id", "5"), ("status", "working")]));
        s.reconcile(manifest(&[(0, 5)]));
        assert_eq!(s.agents.len(), 1, "agent on a live pane must survive");
        assert_eq!(s.agents[0].tab, Some(0));
    }

    #[test]
    fn agent_on_dead_pane_is_culled() {
        let mut s = State { permissions_granted: true, ..Default::default() };
        s.handle_status(&args(&[("pane_id", "5"), ("status", "working")]));
        s.reconcile(manifest(&[(0, 99)]));
        assert!(s.agents.is_empty(), "pane gone -> agent removed");
    }

    #[test]
    fn pipe_before_first_pane_update_is_not_culled() {
        let mut s = State { permissions_granted: true, ..Default::default() };
        s.handle_status(&args(&[("pane_id", "5"), ("status", "waiting")]));
        s.reconcile(PaneManifest { panes: std::collections::HashMap::new() });
        assert_eq!(s.agents.len(), 1, "empty manifest must not cull agents");
    }

    #[test]
    fn plugin_only_manifest_does_not_cull() {
        let mut s = State { permissions_granted: true, ..Default::default() };
        s.handle_status(&args(&[("pane_id", "5"), ("status", "waiting")]));
        let mut m = PaneManifest { panes: std::collections::HashMap::new() };
        let p = PaneInfo {
            id: 5,
            is_plugin: true,
            ..Default::default()
        };
        m.panes.insert(0, vec![p]);
        s.reconcile(m);
        assert_eq!(s.agents.len(), 1, "plugin panes are not agent panes");
    }

    /// `Text::color_range` takes BYTE offsets, not char offsets.
    #[test]
    fn icon_byte_offset_lands_on_the_icon() {
        for (marker, idx, icon) in [("\u{25b6}", 1usize, "\u{25cf}"), (" ", 2, "\u{2713}"), ("\u{25b6}", 10, "\u{280b}")] {
            let line = format!("{} {} {} rest", marker, idx, icon);
            let start = marker.len() + 1 + idx.to_string().len() + 1;
            let end = start + icon.len() - 1;
            assert!(line.is_char_boundary(start), "start must be a char boundary");
            assert!(line.is_char_boundary(end + 1), "end+1 must be a char boundary");
            assert_eq!(&line[start..=end], icon, "range must cover exactly the icon");
        }
    }
}
