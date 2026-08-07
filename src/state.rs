//! Panel state: pipe handling, pane reconciliation.

use std::collections::BTreeMap;
use zellij_tile::prelude::*;

use crate::agent::Agent;
use crate::host;
use crate::install::Install;
use crate::status::Status;
use crate::{SPINNER, TICK};

/// A permission prompt parked by a blocked hook, waiting on a verdict.
pub(crate) struct Ask {
    pub(crate) pane_id: u32,
    pub(crate) verdict_file: String,
    pub(crate) tool_name: String,
    pub(crate) tool_arg: String,
}

#[derive(Default)]
pub struct State {
    pub(crate) agents: Vec<Agent>,
    /// At most one prompt is shown at a time, for the selected agent.
    pub(crate) asks: Vec<Ask>,
    pub(crate) selected: usize,
    pub(crate) frame: usize,
    pub(crate) now: f64,
    pub(crate) permissions_granted: bool,
    pub(crate) kill_armed: Option<u32>,
    pub(crate) timer_running: bool,
    pub(crate) popup_on_waiting: bool,
    pub(crate) hidden: bool,
    pub(crate) install: Install,
    /// Needed to scope the scan; the plugin is not told this at load, it
    /// arrives with the first `SessionUpdate`.
    pub(crate) session_name: String,
    /// A scan is in flight, so a second one would be wasted work.
    pub(crate) scan_pending: bool,
}

impl State {
    /// The setup prompt replaces the empty screen when nothing can report in.
    /// Once an agent has checked in the hooks demonstrably work, so the prompt
    /// is suppressed regardless of what the last status read said.
    pub(crate) fn showing_setup(&self) -> bool {
        !self.install.open && self.agents.is_empty() && self.install.needs_setup()
    }

    pub(crate) fn icon_for(&self, agent: &Agent) -> &'static str {
        match agent.status {
            Status::Working | Status::Compact => SPINNER[self.frame % SPINNER.len()],
            Status::Waiting => "\u{25cf}",
            Status::IdleWait => "\u{25d0}",
            Status::Failed => "\u{2717}",
            Status::Done => "\u{2713}",
            Status::Idle => "\u{25cb}",
            Status::Discovered => "\u{25cc}",
        }
    }

    /// Failed, waiting, working, done. `idle-wait` counts as waiting: both mean
    /// the agent is blocked on you.
    ///
    /// `Discovered` is counted as nothing, like `Idle`. Folding it into any
    /// bucket would state something the scan cannot know - it found a process,
    /// not a status - and the header is the one place that must not guess.
    pub(crate) fn counts(&self) -> (usize, usize, usize, usize) {
        let mut c = (0, 0, 0, 0);
        for a in &self.agents {
            match a.status {
                Status::Failed => c.0 += 1,
                Status::Waiting | Status::IdleWait => c.1 += 1,
                Status::Working | Status::Compact => c.2 += 1,
                Status::Done => c.3 += 1,
                Status::Idle | Status::Discovered => {}
            }
        }
        c
    }

    /// Agents found by the scan that have never reported.
    pub(crate) fn discovered_count(&self) -> usize {
        self.agents.iter().filter(|a| a.status == Status::Discovered).count()
    }

    pub(crate) fn arm_timer(&mut self) {
        if !self.timer_running && self.agents.iter().any(|a| a.status.is_active()) {
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
            self.asks.retain(|k| k.pane_id != pane_id);
            self.clamp_selection();
            return self.agents.len() != before;
        }

        // Subagent and task events carry no status: they adjust counters on a row
        // that already exists rather than describing the pane's own state.
        if raw_status.is_empty() {
            return self.handle_counters(pane_id, args);
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
            // A discovered row's tool came from the executable name; the hook
            // reporting in is authoritative and replaces it.
            if agent.status == Status::Discovered {
                if let Some(tool) = args.get("tool").filter(|t| !t.is_empty()) {
                    agent.tool = tool.clone();
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
            if let Some(m) = args.get("perm_mode") {
                agent.perm_mode = m.clone();
            }
            // The agent moved on by itself, so the parked prompt is moot: the
            // hook timed out and fell through to its own prompt.
            if changed && status != Status::Waiting {
                self.asks.retain(|k| k.pane_id != pane_id);
            }
            // A new turn retires the previous turn's fan-out and task list.
            if changed && status == Status::Working {
                agent.subagents = 0;
                agent.subagent_types.clear();
                agent.tasks_total = 0;
                agent.tasks_done = 0;
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
                perm_mode: args.get("perm_mode").cloned().unwrap_or_default(),
                subagents: 0,
                subagent_types: Vec::new(),
                tasks_total: 0,
                tasks_done: 0,
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

    /// Applies a subagent / task-progress delta to an existing row. Counters are
    /// sent as deltas because the hook is stateless.
    fn handle_counters(&mut self, pane_id: u32, args: &BTreeMap<String, String>) -> bool {
        let delta = |k: &str| args.get(k).and_then(|v| v.parse::<i32>().ok()).unwrap_or(0);
        let (sub, created, done) = (delta("subagent_delta"), delta("task_delta"), delta("task_done_delta"));
        if sub == 0 && created == 0 && done == 0 {
            return false;
        }
        let Some(agent) = self.agents.iter_mut().find(|a| a.pane_id == pane_id) else {
            return false;
        };
        agent.subagents = agent.subagents.saturating_add_signed(sub);
        agent.tasks_total = agent.tasks_total.saturating_add_signed(created);
        agent.tasks_done = agent.tasks_done.saturating_add_signed(done);
        if sub > 0 {
            if let Some(t) = args.get("agent_type").filter(|t| !t.is_empty()) {
                if !agent.subagent_types.contains(t) {
                    agent.subagent_types.push(t.clone());
                }
            }
        }
        if agent.subagents == 0 {
            agent.subagent_types.clear();
        }
        true
    }

    /// A blocked hook parking a permission prompt. Replaces any earlier ask for
    /// the same pane: only the newest can still be answered.
    pub(crate) fn handle_ask(&mut self, args: &BTreeMap<String, String>) -> bool {
        let Some(pane_id) = args.get("pane_id").and_then(|v| v.parse::<u32>().ok()) else {
            return false;
        };
        let Some(verdict_file) = args.get("verdict_file").filter(|f| !f.is_empty()) else {
            return false;
        };
        self.asks.retain(|a| a.pane_id != pane_id);
        self.asks.push(Ask {
            pane_id,
            verdict_file: verdict_file.clone(),
            tool_name: args.get("tool_name").cloned().unwrap_or_default(),
            tool_arg: args.get("tool_arg").cloned().unwrap_or_default(),
        });
        true
    }

    pub(crate) fn ask_for(&self, pane_id: u32) -> Option<&Ask> {
        self.asks.iter().find(|a| a.pane_id == pane_id)
    }

    /// Writes the verdict the hook is polling for. The hook treats a missing
    /// file as "no answer" and falls through to its own prompt, so a failed
    /// write degrades to the normal flow rather than wedging the turn.
    pub(crate) fn answer_selected(&mut self, allow: bool) -> bool {
        let Some(pane_id) = self.agents.get(self.selected).map(|a| a.pane_id) else {
            return false;
        };
        let Some(ask) = self.asks.iter().find(|a| a.pane_id == pane_id) else {
            return false;
        };
        let verdict = if allow { "allow" } else { "deny" };
        host::write_verdict(&ask.verdict_file, verdict);
        self.asks.retain(|a| a.pane_id != pane_id);
        if let Some(agent) = self.agents.iter_mut().find(|a| a.pane_id == pane_id) {
            agent.status = Status::Working;
            agent.status_since = self.now;
            agent.detail = Some(match allow {
                true => "approved from panel".to_string(),
                false => "rejected from panel".to_string(),
            });
        }
        self.sort_agents();
        self.arm_timer();
        true
    }

    /// Runs a scan unless one is already in flight or the session name is not
    /// known yet - without it the scan cannot be scoped and would list agents
    /// from every session on the machine.
    pub(crate) fn request_scan(&mut self) {
        if self.scan_pending || self.session_name.is_empty() || !self.permissions_granted {
            return;
        }
        self.scan_pending = true;
        crate::discover::dispatch(&self.session_name);
    }

    /// Merges a process scan into the agent list.
    ///
    /// Discovery only ever *adds* rows for panes nothing has reported for. A
    /// pane that already has an agent keeps everything it reported: the scan
    /// knows strictly less than a hook does, so letting it write would
    /// downgrade a live row to `found`.
    pub(crate) fn apply_scan(&mut self, found: Vec<crate::discover::Found>) -> bool {
        // A discovered row has no `ended` event coming for it, so a process that
        // exited is dropped by its absence from the next scan. Hook-reported
        // rows are untouched: their lifecycle is owned by the hook and by
        // `reconcile`, and a scan that misses one must not delete it.
        let before = self.agents.len();
        self.agents
            .retain(|a| a.status != Status::Discovered || found.iter().any(|f| f.pane_id == a.pane_id));
        let mut changed = self.agents.len() != before;

        for f in found {
            if self.agents.iter().any(|a| a.pane_id == f.pane_id) {
                continue;
            }
            self.agents.push(Agent {
                pane_id: f.pane_id,
                tool: f.tool,
                session_id: String::new(),
                status: Status::Discovered,
                cwd: String::new(),
                task: None,
                detail: None,
                turns: 0,
                status_since: self.now,
                tab: None,
                pane_title: String::new(),
                alive: true,
                perm_mode: String::new(),
                subagents: 0,
                subagent_types: Vec::new(),
                tasks_total: 0,
                tasks_done: 0,
            });
            changed = true;
        }
        if changed {
            self.clamp_selection();
            self.sort_agents();
        }
        changed
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
            // A prompt whose pane is gone can never be answered.
            self.asks.retain(|k| self.agents.iter().any(|a| a.pane_id == k.pane_id));
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
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
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
        for s in ["working", "waiting", "done", "idle", "idlewait", "compact", "failed"] {
            assert!(Status::parse(s).is_some(), "{} must parse", s);
        }
        assert!(Status::parse("ended").is_none());
        assert!(Status::parse("garbage").is_none());
    }

    /// A stopped agent outranks a blocked one, and both outrank progress.
    #[test]
    fn failed_sorts_above_waiting() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "working")]));
        s.handle_status(&args(&[("pane_id", "2"), ("status", "waiting")]));
        s.handle_status(&args(&[("pane_id", "3"), ("status", "failed")]));
        s.handle_status(&args(&[("pane_id", "4"), ("status", "idlewait")]));
        let order: Vec<&str> = s.agents.iter().map(|a| a.status.label()).collect();
        assert_eq!(order, vec!["failed", "waiting", "idle-wait", "working"]);
    }

    #[test]
    fn counts_split_failed_and_fold_idlewait_into_waiting() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "failed")]));
        s.handle_status(&args(&[("pane_id", "2"), ("status", "waiting")]));
        s.handle_status(&args(&[("pane_id", "3"), ("status", "idlewait")]));
        s.handle_status(&args(&[("pane_id", "4"), ("status", "compact")]));
        s.handle_status(&args(&[("pane_id", "5"), ("status", "done")]));
        assert_eq!(
            s.counts(),
            (1, 2, 1, 1),
            "idle-wait counts as waiting, compact as working"
        );
    }

    /// Only `default` is suppressed; a risky mode must reach the row.
    #[test]
    fn perm_mode_is_carried_and_updated() {
        let mut s = state();
        s.handle_status(&args(&[
            ("pane_id", "1"),
            ("status", "working"),
            ("perm_mode", "bypassPermissions"),
        ]));
        assert_eq!(s.agents[0].perm_mode, "bypassPermissions");
        s.handle_status(&args(&[("pane_id", "1"), ("status", "working"), ("perm_mode", "")]));
        assert_eq!(s.agents[0].perm_mode, "", "hook sends empty for default mode");
    }

    #[test]
    fn subagent_deltas_accumulate_and_drain() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "working")]));
        for t in ["Explore", "Plan"] {
            s.handle_status(&args(&[
                ("pane_id", "1"),
                ("status", ""),
                ("subagent_delta", "1"),
                ("agent_type", t),
            ]));
        }
        assert_eq!(s.agents[0].subagents, 2);
        assert_eq!(s.agents[0].subagent_types, vec!["Explore", "Plan"]);

        s.handle_status(&args(&[("pane_id", "1"), ("status", ""), ("subagent_delta", "-1")]));
        assert_eq!(s.agents[0].subagents, 1);
        s.handle_status(&args(&[("pane_id", "1"), ("status", ""), ("subagent_delta", "-1")]));
        assert_eq!(s.agents[0].subagents, 0);
        assert!(
            s.agents[0].subagent_types.is_empty(),
            "types clear once the fan-out drains"
        );
    }

    /// A stray extra Stop must not underflow the counter into a huge number.
    #[test]
    fn subagent_delta_saturates_at_zero() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "working")]));
        s.handle_status(&args(&[("pane_id", "1"), ("status", ""), ("subagent_delta", "-1")]));
        assert_eq!(s.agents[0].subagents, 0);
    }

    #[test]
    fn duplicate_subagent_type_is_not_listed_twice() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "working")]));
        for _ in 0..2 {
            s.handle_status(&args(&[
                ("pane_id", "1"),
                ("status", ""),
                ("subagent_delta", "1"),
                ("agent_type", "Explore"),
            ]));
        }
        assert_eq!(s.agents[0].subagents, 2);
        assert_eq!(s.agents[0].subagent_types, vec!["Explore"]);
    }

    #[test]
    fn task_progress_accumulates() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "working")]));
        for _ in 0..3 {
            s.handle_status(&args(&[("pane_id", "1"), ("status", ""), ("task_delta", "1")]));
        }
        s.handle_status(&args(&[("pane_id", "1"), ("status", ""), ("task_done_delta", "1")]));
        assert_eq!((s.agents[0].tasks_total, s.agents[0].tasks_done), (3, 1));
    }

    /// Counters describe one turn; carrying them forward would show a stale
    /// fan-out against the next prompt.
    #[test]
    fn new_turn_resets_counters() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "working")]));
        s.handle_status(&args(&[
            ("pane_id", "1"),
            ("status", ""),
            ("subagent_delta", "1"),
            ("agent_type", "Explore"),
        ]));
        s.handle_status(&args(&[("pane_id", "1"), ("status", ""), ("task_delta", "2")]));
        s.handle_status(&args(&[("pane_id", "1"), ("status", "done")]));
        s.handle_status(&args(&[("pane_id", "1"), ("status", "working")]));
        assert_eq!(s.agents[0].subagents, 0);
        assert_eq!(s.agents[0].tasks_total, 0);
        assert!(s.agents[0].subagent_types.is_empty());
    }

    /// Counter events name no status, so they must never create a row.
    #[test]
    fn counter_pipe_for_unknown_pane_is_ignored() {
        let mut s = state();
        assert!(!s.handle_status(&args(&[("pane_id", "99"), ("status", ""), ("subagent_delta", "1")])));
        assert!(s.agents.is_empty());
    }

    #[test]
    fn empty_status_with_no_deltas_is_ignored() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "working")]));
        assert!(!s.handle_status(&args(&[("pane_id", "1"), ("status", "")])));
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
        assert!(
            !s.handle_label(&args(&[("pane_id", "99"), ("label", "x")])),
            "unknown pane"
        );
    }

    fn ask(pane: &str) -> BTreeMap<String, String> {
        args(&[
            ("pane_id", pane),
            ("verdict_file", "/tmp/zj/v"),
            ("tool_name", "Bash"),
            ("tool_arg", "rm -rf node_modules"),
        ])
    }

    #[test]
    fn ask_is_recorded_for_its_pane() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "waiting")]));
        assert!(s.handle_ask(&ask("1")));
        assert_eq!(s.ask_for(1).map(|a| a.tool_arg.as_str()), Some("rm -rf node_modules"));
        assert!(s.ask_for(2).is_none());
    }

    /// Without a verdict file the hook has nowhere to read an answer from, so
    /// showing the prompt would offer an action that cannot work.
    #[test]
    fn ask_without_a_verdict_file_is_rejected() {
        let mut s = state();
        assert!(!s.handle_ask(&args(&[("pane_id", "1")])));
        assert!(!s.handle_ask(&args(&[("pane_id", "1"), ("verdict_file", "")])));
        assert!(s.asks.is_empty());
    }

    #[test]
    fn a_second_ask_replaces_the_first_for_that_pane() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "waiting")]));
        s.handle_ask(&ask("1"));
        let mut second = ask("1");
        second.insert("tool_arg".into(), "git push --force".into());
        s.handle_ask(&second);
        assert_eq!(s.asks.len(), 1, "one prompt per pane");
        assert_eq!(s.ask_for(1).map(|a| a.tool_arg.as_str()), Some("git push --force"));
    }

    #[test]
    fn answering_clears_the_prompt_and_resumes_the_agent() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "waiting")]));
        s.handle_ask(&ask("1"));
        assert!(s.answer_selected(true));
        assert!(s.ask_for(1).is_none());
        assert!(s.agents[0].status == Status::Working);
        assert_eq!(s.agents[0].detail.as_deref(), Some("approved from panel"));
    }

    #[test]
    fn rejecting_is_recorded_distinctly() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "waiting")]));
        s.handle_ask(&ask("1"));
        assert!(s.answer_selected(false));
        assert_eq!(s.agents[0].detail.as_deref(), Some("rejected from panel"));
    }

    /// Pressing approve with no prompt parked must do nothing at all.
    #[test]
    fn answering_without_a_prompt_is_a_no_op() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "working")]));
        assert!(!s.answer_selected(true));
        assert!(!s.answer_selected(false));
    }

    /// The hook gives up after its timeout and prompts in-pane instead; a stale
    /// box in the panel would offer an answer nothing is listening for.
    #[test]
    fn agent_moving_on_clears_a_stale_prompt() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "waiting")]));
        s.handle_ask(&ask("1"));
        s.handle_status(&args(&[("pane_id", "1"), ("status", "working")]));
        assert!(s.ask_for(1).is_none());
    }

    #[test]
    fn session_end_clears_a_pending_prompt() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "waiting")]));
        s.handle_ask(&ask("1"));
        s.handle_status(&args(&[("pane_id", "1"), ("status", "ended")]));
        assert!(s.asks.is_empty());
    }

    fn found(pairs: &[(u32, &str)]) -> Vec<crate::discover::Found> {
        pairs
            .iter()
            .map(|(pane_id, tool)| crate::discover::Found {
                pane_id: *pane_id,
                tool: tool.to_string(),
            })
            .collect()
    }

    #[test]
    fn scan_adds_rows_for_silent_agents() {
        let mut s = state();
        assert!(s.apply_scan(found(&[(2, "claude"), (6, "codex")])));
        assert_eq!(s.agents.len(), 2);
        assert!(s.agents.iter().all(|a| a.status == Status::Discovered));
        assert_eq!(s.discovered_count(), 2);
    }

    /// The reported row already knows more than the scan does, so a scan over it
    /// must neither duplicate it nor overwrite what the hook said.
    #[test]
    fn scan_does_not_duplicate_or_downgrade_a_reported_agent() {
        let mut s = state();
        s.handle_status(&args(&[
            ("pane_id", "3"),
            ("status", "working"),
            ("task", "Fix the parser"),
        ]));
        assert!(!s.apply_scan(found(&[(3, "claude")])), "nothing changed");
        assert_eq!(s.agents.len(), 1, "one row per pane");
        assert!(s.agents[0].status == Status::Working, "hook status wins");
        assert_eq!(s.agents[0].task.as_deref(), Some("Fix the parser"));
    }

    /// The other order: discovery first, then the agent finally acts.
    #[test]
    fn hook_upgrades_a_discovered_row_in_place() {
        let mut s = state();
        s.apply_scan(found(&[(3, "claude")]));
        assert_eq!(s.discovered_count(), 1);

        s.handle_status(&args(&[
            ("pane_id", "3"),
            ("status", "waiting"),
            ("task", "Approve this"),
            ("tool", "codex"),
        ]));
        assert_eq!(s.agents.len(), 1, "upgrade must not add a second row");
        assert!(s.agents[0].status == Status::Waiting);
        assert_eq!(s.agents[0].task.as_deref(), Some("Approve this"));
        assert_eq!(s.agents[0].tool, "codex", "the hook is authoritative on tool");
        assert_eq!(s.discovered_count(), 0);
    }

    /// No `ended` event ever arrives for a discovered row, so the only evidence
    /// the process is gone is its absence from the next scan.
    #[test]
    fn discovered_row_disappears_when_its_process_exits() {
        let mut s = state();
        s.apply_scan(found(&[(2, "claude"), (3, "claude")]));
        assert_eq!(s.agents.len(), 2);
        assert!(s.apply_scan(found(&[(2, "claude")])));
        assert_eq!(s.agents.len(), 1);
        assert_eq!(s.agents[0].pane_id, 2);
    }

    /// A scan that returns nothing must not wipe agents the hooks reported.
    #[test]
    fn empty_scan_never_culls_reported_agents() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "working")]));
        s.apply_scan(found(&[(2, "claude")]));
        assert_eq!(s.agents.len(), 2);

        assert!(s.apply_scan(Vec::new()), "the discovered row drops");
        assert_eq!(s.agents.len(), 1, "the reported one stays");
        assert_eq!(s.agents[0].pane_id, 1);
    }

    /// Discovery states nothing about what an agent is doing, so folding it into
    /// a header bucket would make the summary line assert what it cannot know.
    #[test]
    fn discovered_agents_are_absent_from_the_header_counts() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "waiting")]));
        s.apply_scan(found(&[(2, "claude"), (3, "claude")]));
        assert_eq!(s.counts(), (0, 1, 0, 0));
        assert_eq!(s.discovered_count(), 2);
    }

    /// Least urgent: nothing is known about it.
    #[test]
    fn discovered_sorts_last() {
        let mut s = state();
        s.apply_scan(found(&[(1, "claude")]));
        s.handle_status(&args(&[("pane_id", "2"), ("status", "idle")]));
        s.handle_status(&args(&[("pane_id", "3"), ("status", "waiting")]));
        let order: Vec<&str> = s.agents.iter().map(|a| a.status.label()).collect();
        assert_eq!(order, vec!["waiting", "idle", "found"]);
    }

    /// A shrinking list must not strand the cursor past the end.
    #[test]
    fn selection_is_clamped_when_a_scan_removes_rows() {
        let mut s = state();
        s.apply_scan(found(&[(1, "claude"), (2, "claude"), (3, "claude")]));
        s.selected = 2;
        s.apply_scan(found(&[(1, "claude")]));
        assert!(s.selected < s.agents.len(), "selection must stay in bounds");
    }

    /// No hook may claim a state that asserts the opposite of what it means.
    #[test]
    fn discovered_is_not_reachable_from_a_pipe() {
        assert!(Status::parse("found").is_none());
        assert!(Status::parse("discovered").is_none());
    }

    #[test]
    fn counts_summarize_by_status() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "waiting")]));
        s.handle_status(&args(&[("pane_id", "2"), ("status", "working")]));
        s.handle_status(&args(&[("pane_id", "3"), ("status", "working")]));
        s.handle_status(&args(&[("pane_id", "4"), ("status", "done")]));
        s.handle_status(&args(&[("pane_id", "5"), ("status", "idle")]));
        assert_eq!(s.counts(), (0, 1, 2, 1));
    }
}

#[cfg(test)]
mod reconcile_tests {
    use super::*;

    fn args(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn manifest(panes: &[(usize, u32)]) -> PaneManifest {
        let mut m = PaneManifest {
            panes: std::collections::HashMap::new(),
        };
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
        let mut s = State {
            permissions_granted: true,
            ..Default::default()
        };
        s.handle_status(&args(&[("pane_id", "5"), ("status", "working")]));
        s.reconcile(manifest(&[(0, 5)]));
        assert_eq!(s.agents.len(), 1, "agent on a live pane must survive");
        assert_eq!(s.agents[0].tab, Some(0));
    }

    #[test]
    fn agent_on_dead_pane_is_culled() {
        let mut s = State {
            permissions_granted: true,
            ..Default::default()
        };
        s.handle_status(&args(&[("pane_id", "5"), ("status", "working")]));
        s.reconcile(manifest(&[(0, 99)]));
        assert!(s.agents.is_empty(), "pane gone -> agent removed");
    }

    #[test]
    fn pipe_before_first_pane_update_is_not_culled() {
        let mut s = State {
            permissions_granted: true,
            ..Default::default()
        };
        s.handle_status(&args(&[("pane_id", "5"), ("status", "waiting")]));
        s.reconcile(PaneManifest {
            panes: std::collections::HashMap::new(),
        });
        assert_eq!(s.agents.len(), 1, "empty manifest must not cull agents");
    }

    #[test]
    fn plugin_only_manifest_does_not_cull() {
        let mut s = State {
            permissions_granted: true,
            ..Default::default()
        };
        s.handle_status(&args(&[("pane_id", "5"), ("status", "waiting")]));
        let mut m = PaneManifest {
            panes: std::collections::HashMap::new(),
        };
        let p = PaneInfo {
            id: 5,
            is_plugin: true,
            ..Default::default()
        };
        m.panes.insert(0, vec![p]);
        s.reconcile(m);
        assert_eq!(s.agents.len(), 1, "plugin panes are not agent panes");
    }

    /// `Text::color_range` takes CHAR offsets, not byte offsets. Byte offsets
    /// slide the range past the multi-byte marker and colour part of the next
    /// column instead of the icon.
    #[test]
    fn icon_char_offset_lands_on_the_icon() {
        for (marker, idx, icon) in [
            ("\u{25b6}", 1usize, "\u{25cf}"),
            (" ", 2, "\u{2713}"),
            ("\u{25b6}", 10, "\u{280b}"),
        ] {
            let line = format!("{} {} {} rest", marker, idx, icon);
            let start = crate::style::chars(marker) + 1 + idx.to_string().len() + 1;
            let end = start + crate::style::chars(icon);
            let covered: String = line.chars().skip(start).take(end - start).collect();
            assert_eq!(covered, icon, "range must cover exactly the icon in {:?}", line);
        }
    }
}
