//! Panel state: pipe handling, pane reconciliation.

use std::collections::BTreeMap;
use zellij_tile::prelude::*;

use crate::agent::{Agent, AgentId};
use crate::host;
use crate::install::Install;
use crate::status::Status;
use crate::{SPINNER, STALE_AFTER, TICK};

/// A permission prompt parked by a blocked hook, waiting on a verdict.
pub(crate) struct Ask {
    pub(crate) id: AgentId,
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
    pub(crate) kill_armed: Option<AgentId>,
    pub(crate) timer_running: bool,
    pub(crate) popup_on_waiting: bool,
    pub(crate) hidden: bool,
    pub(crate) install: Install,
    /// The panel's own session; rows from anywhere else are foreign.
    pub(crate) session_name: String,
    /// Every session Zellij currently lists, used to spot dead ones.
    pub(crate) live_sessions: Vec<String>,
    /// A scan is in flight, so a second one would be wasted work.
    pub(crate) scan_pending: bool,
    /// Set by the first successful scan. Until then the panel has piped rows and
    /// no cross-session evidence at all, so culling would wipe them.
    pub(crate) scan_completed: bool,
    /// The process scan, off until `load()` reads the `discover` key (default
    /// on). A bool that defaults false is deliberate: `register_plugin!` builds
    /// the state with `Default`, and `load()` always runs before any event.
    pub(crate) discover: bool,
    /// Newest spool timestamp seen, the reference point ages are measured from.
    /// The plugin has no wall clock, so records are dated relative to each
    /// other rather than to a host time it cannot read.
    pub(crate) spool_epoch: f64,
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
            Status::Unknown => "?",
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
                Status::Idle | Status::Discovered | Status::Unknown => {}
            }
        }
        c
    }

    /// Agents found by the scan that have never reported.
    pub(crate) fn discovered_count(&self) -> usize {
        self.agents.iter().filter(|a| a.status == Status::Discovered).count()
    }

    pub(crate) fn arm_timer(&mut self) {
        if self.timer_running {
            return;
        }
        // Foreign rows also need ticks, or one left mid-decay never reaches
        // `unknown` because nothing else advances `now`.
        let home = &self.session_name;
        let needed = self
            .agents
            .iter()
            .any(|a| a.status.is_active() || (a.id.session != *home && a.status.is_reported()));
        if needed {
            self.timer_running = true;
            host::set_timeout(TICK);
        }
    }

    /// Messages from before the hook carried `session=` fall back to the
    /// panel's own session, which is where they must have come from.
    fn id_from(&self, args: &BTreeMap<String, String>) -> Option<AgentId> {
        let pane_id = args.get("pane_id").and_then(|v| v.parse::<u32>().ok())?;
        let session = args
            .get("session")
            .filter(|s| !s.is_empty())
            .cloned()
            .unwrap_or_else(|| self.session_name.clone());
        Some(AgentId { session, pane_id })
    }

    /// Rows whose session is gone go `unknown` rather than disappearing.
    pub(crate) fn apply_sessions(&mut self, live: Vec<String>) -> bool {
        if live.is_empty() {
            return false;
        }
        self.live_sessions = live;
        let mut changed = false;
        for agent in self.agents.iter_mut() {
            let alive = self.live_sessions.contains(&agent.id.session);
            if agent.session_alive != alive {
                agent.session_alive = alive;
                changed = true;
            }
            if !alive && agent.status != Status::Unknown {
                agent.status = Status::Unknown;
                changed = true;
            }
        }
        if changed {
            self.sort_agents();
        }
        changed
    }

    /// A foreign row's status is a snapshot: the hook only pipes into its own
    /// session, so nothing refreshes it. Past `STALE_AFTER` the panel says
    /// `unknown` rather than keeping a `working` it can no longer vouch for.
    pub(crate) fn age_foreign_rows(&mut self) -> bool {
        let (now, home) = (self.now, self.session_name.clone());
        let mut changed = false;
        for agent in self.agents.iter_mut() {
            let stale = now - agent.last_report >= STALE_AFTER;
            if agent.id.session != home && stale && agent.status.is_reported() {
                agent.status = Status::Unknown;
                agent.status_since = now;
                changed = true;
            }
        }
        if changed {
            self.sort_agents();
        }
        changed
    }

    pub(crate) fn handle_status(&mut self, args: &BTreeMap<String, String>) -> bool {
        let Some(id) = self.id_from(args) else {
            return false;
        };
        let raw_status = args.get("status").map(|s| s.as_str()).unwrap_or("");

        if raw_status == "ended" {
            let before = self.agents.len();
            self.agents.retain(|a| a.id != id);
            self.asks.retain(|k| k.id != id);
            self.clamp_selection();
            return self.agents.len() != before;
        }

        // Subagent and task events carry no status: they adjust counters on a row
        // that already exists rather than describing the pane's own state.
        if raw_status.is_empty() {
            return self.handle_counters(&id, args);
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
        if let Some(agent) = self.agents.iter_mut().find(|a| a.id == id) {
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
                self.asks.retain(|k| k.id != id);
            }
            // A new turn retires the previous turn's fan-out and task list.
            if changed && status == Status::Working {
                agent.subagents = 0;
                agent.subagent_types.clear();
                agent.tasks_total = 0;
                agent.tasks_done = 0;
            }
            agent.alive = true;
            agent.session_alive = true;
            agent.last_report = now;
        } else {
            newly_waiting = status == Status::Waiting;
            self.agents.push(Agent {
                id: id.clone(),
                tool: args.get("tool").cloned().unwrap_or_else(|| "agent".into()),
                session_id: args.get("session_id").cloned().unwrap_or_default(),
                status,
                cwd: args.get("cwd").cloned().unwrap_or_default(),
                task,
                detail,
                turns: if status == Status::Working { 1 } else { 0 },
                status_since: now,
                last_report: now,
                spool_ts: 0.0,
                tab: None,
                pane_title: String::new(),
                alive: true,
                perm_mode: args.get("perm_mode").cloned().unwrap_or_default(),
                subagents: 0,
                subagent_types: Vec::new(),
                tasks_total: 0,
                tasks_done: 0,
                session_alive: true,
            });
        }

        self.sort_agents();
        self.arm_timer();

        if newly_waiting && self.popup_on_waiting && self.hidden {
            if let Some(idx) = self.agents.iter().position(|a| a.id == id) {
                self.selected = idx;
            }
            self.hidden = false;
            host::show_self(true);
        }
        true
    }

    /// Applies a subagent / task-progress delta to an existing row. Counters are
    /// sent as deltas because the hook is stateless.
    fn handle_counters(&mut self, id: &AgentId, args: &BTreeMap<String, String>) -> bool {
        let delta = |k: &str| args.get(k).and_then(|v| v.parse::<i32>().ok()).unwrap_or(0);
        let (sub, created, done) = (delta("subagent_delta"), delta("task_delta"), delta("task_done_delta"));
        if sub == 0 && created == 0 && done == 0 {
            return false;
        }
        let Some(agent) = self.agents.iter_mut().find(|a| &a.id == id) else {
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
        let Some(id) = self.id_from(args) else {
            return false;
        };
        let Some(verdict_file) = args.get("verdict_file").filter(|f| !f.is_empty()) else {
            return false;
        };
        self.asks.retain(|a| a.id != id);
        self.asks.push(Ask {
            id,
            verdict_file: verdict_file.clone(),
            tool_name: args.get("tool_name").cloned().unwrap_or_default(),
            tool_arg: args.get("tool_arg").cloned().unwrap_or_default(),
        });
        true
    }

    pub(crate) fn ask_for(&self, id: &AgentId) -> Option<&Ask> {
        self.asks.iter().find(|a| &a.id == id)
    }

    /// Writes the verdict the hook is polling for. The hook treats a missing
    /// file as "no answer" and falls through to its own prompt, so a failed
    /// write degrades to the normal flow rather than wedging the turn.
    pub(crate) fn answer_selected(&mut self, allow: bool) -> bool {
        let Some(id) = self.agents.get(self.selected).map(|a| a.id.clone()) else {
            return false;
        };
        let Some(ask) = self.asks.iter().find(|a| a.id == id) else {
            return false;
        };
        let verdict = if allow { "allow" } else { "deny" };
        host::write_verdict(&ask.verdict_file, verdict);
        self.asks.retain(|a| a.id != id);
        if let Some(agent) = self.agents.iter_mut().find(|a| a.id == id) {
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

    /// Runs a scan unless one is already in flight, or discovery is switched off.
    pub(crate) fn request_scan(&mut self) {
        if self.scan_pending || !self.permissions_granted || !self.discover {
            return;
        }
        self.scan_pending = true;
        crate::discover::dispatch();
    }

    /// Merges a process scan into the agent list.
    ///
    /// The scan only ever *adds* rows for panes nothing has reported for: it
    /// knows strictly less than a hook, so letting it write would downgrade a
    /// live row to `found`.
    ///
    /// Culling is asymmetric. Home rows are owned by the hook and `reconcile`,
    /// so only scan-discovered ones are the scan's to remove. Foreign rows have
    /// no such owner - the scan is the only thing that ever sees them exit - so
    /// it culls them regardless of status.
    #[cfg(test)]
    pub(crate) fn apply_scan(&mut self, found: Vec<crate::discover::Found>) -> bool {
        self.apply_scan_result(crate::discover::Scan {
            found,
            spooled: Vec::new(),
            complete: true,
        })
    }

    /// Merges a scan plus the spool records that came back with it.
    ///
    /// Order matters: rows are reconciled against the process list first, then
    /// the spool refines what survived. A spool record never creates a row -
    /// only a live process justifies one - so a stale file cannot resurrect an
    /// agent that exited.
    pub(crate) fn apply_scan_result(&mut self, scan: crate::discover::Scan) -> bool {
        if !scan.complete {
            return false;
        }
        // Advance the reference point before ages are computed, so the newest
        // record in this batch reads as current.
        for rec in &scan.spooled {
            if rec.ts > self.spool_epoch {
                self.spool_epoch = rec.ts;
            }
        }
        let mut changed = self.merge_found(scan.found);
        changed |= self.apply_spool(scan.spooled);
        if changed {
            self.clamp_selection();
            self.sort_agents();
            self.arm_timer();
        }
        changed
    }

    fn merge_found(&mut self, found: Vec<crate::discover::Found>) -> bool {
        let before = self.agents.len();
        let home = self.session_name.clone();
        let cull_foreign = self.scan_completed;
        self.agents.retain(|a| {
            let seen = found
                .iter()
                .any(|f| f.pane_id == a.pane_id() && f.session == a.id.session);
            if a.id.session == home {
                return a.status != Status::Discovered || seen;
            }
            // A dead session's processes are gone, so the scan cannot see them;
            // `apply_sessions` already marked the row `unknown`.
            !cull_foreign || seen || !a.session_alive
        });
        let mut changed = self.agents.len() != before;
        self.scan_completed = true;

        for f in found {
            let id = AgentId {
                session: f.session,
                pane_id: f.pane_id,
            };
            if self.agents.iter().any(|a| a.id == id) {
                continue;
            }
            self.agents.push(Agent {
                id,
                tool: f.tool,
                session_id: String::new(),
                status: Status::Discovered,
                cwd: String::new(),
                task: None,
                detail: None,
                turns: 0,
                status_since: self.now,
                last_report: self.now,
                spool_ts: 0.0,
                tab: None,
                pane_title: String::new(),
                alive: true,
                perm_mode: String::new(),
                subagents: 0,
                subagent_types: Vec::new(),
                tasks_total: 0,
                tasks_done: 0,
                session_alive: true,
            });
            changed = true;
        }
        if changed {
            self.clamp_selection();
            self.sort_agents();
        }
        changed
    }

    /// Applies spool records to rows the process scan already justified.
    ///
    /// Home rows are skipped entirely: the hook pipes straight into this
    /// session, so the pipe is both fresher and authoritative, and letting a
    /// spool read overwrite it would flap the row.
    fn apply_spool(&mut self, spooled: Vec<crate::discover::Spooled>) -> bool {
        let (home, now) = (self.session_name.clone(), self.now);
        let mut changed = false;
        for rec in spooled {
            if rec.session == home {
                continue;
            }
            let id = AgentId {
                session: rec.session,
                pane_id: rec.pane_id,
            };
            let Some(idx) = self.agents.iter().position(|a| a.id == id) else {
                continue;
            };
            // Pane ids are recycled, so a record from a previous agent on this
            // pane must not colour the current one.
            let rec_sid = rec.args.get("session_id").map(String::as_str).unwrap_or("");
            let row_sid = self.agents[idx].session_id.as_str();
            if !rec_sid.is_empty() && !row_sid.is_empty() && rec_sid != row_sid {
                continue;
            }
            let age = self.spool_age(rec.ts);
            if age >= STALE_AFTER {
                continue;
            }
            let Some(status) = rec.args.get("status").and_then(|s| Status::parse(s)) else {
                continue;
            };
            let agent = &mut self.agents[idx];
            // Records are compared in their own epoch units; `last_report` is on
            // the panel's tick clock and the two are not comparable.
            if rec.ts <= agent.spool_ts {
                continue;
            }
            agent.spool_ts = rec.ts;
            let seen_at = now - age;
            if agent.status != status {
                agent.status = status;
                agent.status_since = seen_at;
                changed = true;
            }
            if let Some(t) = rec.args.get("task").filter(|t| !t.is_empty()) {
                if agent.task.as_deref() != Some(t.as_str()) {
                    agent.task = Some(t.clone());
                    changed = true;
                }
            }
            if let Some(d) = rec.args.get("detail").filter(|d| !d.is_empty()) {
                if agent.detail.as_deref() != Some(d.as_str()) {
                    agent.detail = Some(d.clone());
                    changed = true;
                }
            }
            if let Some(c) = rec.args.get("cwd").filter(|c| !c.is_empty()) {
                if agent.cwd != *c {
                    agent.cwd = c.clone();
                    changed = true;
                }
            }
            if let Some(tool) = rec.args.get("tool").filter(|t| !t.is_empty()) {
                if agent.tool != *tool {
                    agent.tool = tool.clone();
                    changed = true;
                }
            }
            if let Some(m) = rec.args.get("perm_mode") {
                if agent.perm_mode != *m {
                    agent.perm_mode = m.clone();
                    changed = true;
                }
            }
            if !rec_sid.is_empty() && agent.session_id != rec_sid {
                agent.session_id = rec_sid.to_string();
            }
            agent.last_report = seen_at;
        }
        changed
    }

    /// Wall-clock age of a spool record, in the panel's own tick units.
    ///
    /// The panel has no clock: `now` counts ticks since load. So the newest
    /// record seen is treated as "now" and everything else measured back from
    /// it, which needs no host time call and is immune to clock skew.
    fn spool_age(&self, ts: f64) -> f64 {
        (self.spool_epoch.max(ts) - ts).max(0.0)
    }

    pub(crate) fn handle_label(&mut self, args: &BTreeMap<String, String>) -> bool {
        let Some(id) = self.id_from(args) else {
            return false;
        };
        let Some(label) = args.get("label") else {
            return false;
        };
        if let Some(agent) = self.agents.iter_mut().find(|a| a.id == id) {
            agent.task = Some(label.clone());
            return true;
        }
        false
    }

    pub(crate) fn reconcile(&mut self, manifest: PaneManifest) {
        let home = self.session_name.clone();
        for agent in self.agents.iter_mut() {
            if agent.id.session == home {
                agent.alive = false;
            }
        }
        let mut saw_any_terminal = false;
        for (tab, panes) in manifest.panes {
            for pane in panes {
                // Agents only run in terminal panes.
                if pane.is_plugin {
                    continue;
                }
                saw_any_terminal = true;
                if let Some(agent) = self
                    .agents
                    .iter_mut()
                    .find(|a| a.pane_id() == pane.id && a.id.session == home)
                {
                    agent.alive = true;
                    agent.tab = Some(tab);
                    agent.pane_title = pane.title.clone();
                }
            }
        }
        // Drop agents whose pane is gone, but only once we've actually seen a
        // terminal pane: a pipe can land before the first PaneUpdate.
        if saw_any_terminal {
            self.agents.retain(|a| a.alive || a.id.session != home);
            // A prompt whose pane is gone can never be answered.
            self.asks.retain(|k| self.agents.iter().any(|a| a.id == k.id));
        }
        self.clamp_selection();
        self.sort_agents();
    }

    pub(crate) fn sort_agents(&mut self) {
        self.agents
            .sort_by(|a, b| a.status.rank().cmp(&b.status.rank()).then(a.id.cmp(&b.id)));
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
            session_name: "mob".into(),
            live_sessions: vec!["mob".into()],
            discover: true,
            ..Default::default()
        }
    }

    fn id(pane_id: u32) -> AgentId {
        AgentId {
            session: "mob".into(),
            pane_id,
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
        assert_eq!(
            s.ask_for(&id(1)).map(|a| a.tool_arg.as_str()),
            Some("rm -rf node_modules")
        );
        assert!(s.ask_for(&id(2)).is_none());
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
        assert_eq!(s.ask_for(&id(1)).map(|a| a.tool_arg.as_str()), Some("git push --force"));
    }

    #[test]
    fn answering_clears_the_prompt_and_resumes_the_agent() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "waiting")]));
        s.handle_ask(&ask("1"));
        assert!(s.answer_selected(true));
        assert!(s.ask_for(&id(1)).is_none());
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
        assert!(s.ask_for(&id(1)).is_none());
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
                session: "mob".to_string(),
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
        assert_eq!(s.agents[0].pane_id(), 2);
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
        assert_eq!(s.agents[0].pane_id(), 1);
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

/// The whole point of keying by (session, pane): two sessions routinely hand out
/// the same pane number, and every one of these collapsed into one row before.
#[cfg(test)]
mod cross_session_tests {
    use super::*;
    use crate::agent::{sanitize_session, AgentId};

    fn state() -> State {
        State {
            permissions_granted: true,
            popup_on_waiting: false,
            session_name: "mob".into(),
            live_sessions: vec!["mob".into(), "other".into()],
            discover: true,
            ..Default::default()
        }
    }

    fn args(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn same_pane_id_in_two_sessions_are_separate_agents() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "3"), ("session", "mob"), ("status", "working")]));
        s.handle_status(&args(&[("pane_id", "3"), ("session", "other"), ("status", "waiting")]));
        assert_eq!(s.agents.len(), 2, "one row per (session, pane)");
        let waiting = s.agents.iter().find(|a| a.session() == "other").unwrap();
        assert_eq!(waiting.status, Status::Waiting);
        let working = s.agents.iter().find(|a| a.session() == "mob").unwrap();
        assert_eq!(working.status, Status::Working, "the foreign row must not overwrite it");
    }

    /// `ended` from one session must not delete the other's identically
    /// numbered pane.
    #[test]
    fn ending_one_session_leaves_the_other() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "3"), ("session", "mob"), ("status", "working")]));
        s.handle_status(&args(&[("pane_id", "3"), ("session", "other"), ("status", "working")]));
        s.handle_status(&args(&[("pane_id", "3"), ("session", "other"), ("status", "ended")]));
        assert_eq!(s.agents.len(), 1);
        assert_eq!(s.agents[0].session(), "mob");
    }

    /// The dangerous one: a pane-only key let an approval answer a different
    /// session's prompt.
    #[test]
    fn a_verdict_answers_only_its_own_session() {
        let mut s = state();
        for sess in ["mob", "other"] {
            s.handle_status(&args(&[("pane_id", "3"), ("session", sess), ("status", "waiting")]));
            s.handle_ask(&args(&[
                ("pane_id", "3"),
                ("session", sess),
                ("verdict_file", &format!("/tmp/verdict.{}.3", sess)),
                ("tool_name", "Bash"),
            ]));
        }
        assert_eq!(s.asks.len(), 2, "one parked prompt per session");

        let target = s.agents.iter().position(|a| a.session() == "mob").unwrap();
        s.selected = target;
        assert!(s.answer_selected(true));

        assert_eq!(s.asks.len(), 1, "only the selected agent's prompt is answered");
        assert_eq!(s.asks[0].id.session, "other", "the other session still waits");
    }

    /// Reconcile only sees the panel's own session's panes, so a missing foreign
    /// pane is absence of evidence, not a dead agent.
    #[test]
    fn reconcile_never_culls_foreign_rows() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "3"), ("session", "other"), ("status", "working")]));
        s.handle_status(&args(&[("pane_id", "4"), ("session", "mob"), ("status", "working")]));

        let mut panes = std::collections::HashMap::new();
        panes.insert(
            0,
            vec![PaneInfo {
                id: 4,
                is_plugin: false,
                title: "claude".into(),
                ..Default::default()
            }],
        );
        s.reconcile(PaneManifest { panes });

        assert_eq!(s.agents.len(), 2, "the foreign row survives a local pane sweep");
        assert!(s.agents.iter().any(|a| a.session() == "other"));
    }

    #[test]
    fn a_dead_session_turns_its_rows_unknown_without_dropping_them() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "3"), ("session", "other"), ("status", "working")]));
        assert!(s.apply_sessions(vec!["mob".into()]));
        assert_eq!(s.agents.len(), 1, "the row persists");
        assert_eq!(s.agents[0].status, Status::Unknown);
        assert!(!s.agents[0].session_alive);
    }

    /// An empty session list means Zellij told us nothing, not that every
    /// session died; acting on it would blank the whole panel.
    #[test]
    fn an_empty_session_list_changes_nothing() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "3"), ("session", "other"), ("status", "working")]));
        assert!(!s.apply_sessions(Vec::new()));
        assert_eq!(s.agents[0].status, Status::Working);
    }

    #[test]
    fn a_session_coming_back_clears_unknown() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "3"), ("session", "other"), ("status", "working")]));
        s.apply_sessions(vec!["mob".into()]);
        assert_eq!(s.agents[0].status, Status::Unknown);
        assert!(s.apply_sessions(vec!["mob".into(), "other".into()]));
        assert!(s.agents[0].session_alive);
    }

    /// Messages predating the `session=` arg must land on the panel's own
    /// session rather than creating a second, unreachable row.
    #[test]
    fn a_message_without_a_session_falls_back_to_home() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "3"), ("status", "working")]));
        assert_eq!(s.agents.len(), 1);
        assert_eq!(s.agents[0].session(), "mob");
    }

    /// The hook folds unusual characters before sending; the plugin gets the raw
    /// name from Zellij and must fold identically or the two never match.
    ///
    /// `tests/hook_e2e.rs` checks this against the hook's own `tr` for real;
    /// these cases pin the shape so a change here fails fast without spawning a
    /// shell.
    #[test]
    fn session_sanitizing_matches_the_hook() {
        assert_eq!(sanitize_session("my session"), "my_session");
        assert_eq!(sanitize_session("a,b=c"), "a_b_c");
        assert_eq!(sanitize_session("../evil"), ".._evil");
        assert_eq!(sanitize_session("mob-2.1_x"), "mob-2.1_x");
        // One underscore per *byte*, matching `tr`: "é" is two bytes, so a
        // char-wise fold would give "caf_" and miss the hook's "caf__".
        assert_eq!(sanitize_session("café"), "caf__");
        assert_eq!(sanitize_session("日本語"), "_________");
    }

    #[test]
    fn scan_rows_from_several_sessions_all_appear() {
        let mut s = state();
        let found = vec![
            crate::discover::Found {
                session: "mob".into(),
                pane_id: 3,
                tool: "claude".into(),
            },
            crate::discover::Found {
                session: "other".into(),
                pane_id: 3,
                tool: "codex".into(),
            },
        ];
        assert!(s.apply_scan(found));
        assert_eq!(s.agents.len(), 2);
    }

    /// A scan only ever reports live sessions, so its absence list must not
    /// delete a discovered row belonging to another session.
    #[test]
    fn a_scan_only_culls_discovered_rows_it_could_have_seen() {
        let mut s = state();
        s.apply_scan(vec![
            crate::discover::Found {
                session: "mob".into(),
                pane_id: 3,
                tool: "claude".into(),
            },
            crate::discover::Found {
                session: "other".into(),
                pane_id: 3,
                tool: "claude".into(),
            },
        ]);
        assert_eq!(s.agents.len(), 2);
        s.apply_scan(vec![crate::discover::Found {
            session: "mob".into(),
            pane_id: 3,
            tool: "claude".into(),
        }]);
        assert_eq!(s.agents.len(), 1, "the vanished process drops");
        assert_eq!(s.agents[0].session(), "mob");
    }

    #[test]
    fn kill_is_refused_for_a_foreign_row() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "3"), ("session", "other"), ("status", "working")]));
        s.selected = 0;
        assert!(!s.can_kill_selected(), "x must not signal another session's pane");

        s.handle_status(&args(&[("pane_id", "4"), ("session", "mob"), ("status", "working")]));
        s.selected = s.agents.iter().position(|a| a.session() == "mob").unwrap();
        assert!(s.can_kill_selected());
    }

    /// Renders the real row-building path with a mixed list, which is what the
    /// panel actually shows: local rows keep their project, foreign rows name
    /// their session, and both are present at once.
    #[test]
    fn a_mixed_list_renders_local_and_foreign_rows_together() {
        use crate::agent::RowCtx;
        use crate::util::testing::item_text;

        let mut s = state();
        s.handle_status(&args(&[
            ("pane_id", "3"),
            ("session", "mob"),
            ("status", "working"),
            ("cwd", "/Users/x/Projects/api"),
            ("task", "local work"),
        ]));
        s.handle_status(&args(&[
            ("pane_id", "3"),
            ("session", "other"),
            ("status", "waiting"),
            ("cwd", "/Users/x/Projects/web"),
            ("task", "foreign work"),
        ]));

        let rendered: Vec<String> = s
            .agents
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let icon = s.icon_for(a);
                item_text(&a.list_item(
                    i,
                    RowCtx {
                        selected: false,
                        icon,
                        now: s.now,
                        cols: 110,
                        show_cwd: true,
                        home: &s.session_name,
                    },
                ))
            })
            .collect();

        assert_eq!(rendered.len(), 2);
        let all = rendered.join("\n");
        assert!(all.contains("local work") && all.contains("foreign work"));
        assert!(all.contains("api"), "the local row keeps its project: {}", all);
        assert!(all.contains("other"), "the foreign row names its session: {}", all);
        assert!(
            !all.contains("web"),
            "the foreign row shows session, not project: {}",
            all
        );
        // Waiting sorts above working, so the foreign row leads.
        assert!(rendered[0].contains("foreign work"), "{:?}", rendered);
    }

    /// `discover false` leaves hook-reported rows untouched and only stops the
    /// scan, so a panel with it set still tracks everything that reports in.
    #[test]
    fn discovery_can_be_switched_off() {
        let mut on = state();
        on.request_scan();
        assert!(on.scan_pending, "the default must actually scan");

        let mut s = state();
        s.discover = false;
        s.request_scan();
        assert!(!s.scan_pending, "no scan should be dispatched");

        s.handle_status(&args(&[("pane_id", "3"), ("session", "mob"), ("status", "working")]));
        assert_eq!(s.agents.len(), 1, "hook reports still land");
    }

    fn found(pairs: &[(&str, u32)]) -> Vec<crate::discover::Found> {
        pairs
            .iter()
            .map(|(session, pane_id)| crate::discover::Found {
                session: session.to_string(),
                pane_id: *pane_id,
                tool: "claude".to_string(),
            })
            .collect()
    }

    /// The reported symptom: a panel that learned an agent by pipe and one that
    /// learned it by scan must agree after seeing the same scan.
    #[test]
    fn two_panels_with_different_histories_converge() {
        let mut by_pipe = state();
        by_pipe.handle_status(&args(&[("pane_id", "3"), ("session", "other"), ("status", "working")]));
        let mut by_scan = state();

        for s in [&mut by_pipe, &mut by_scan] {
            s.apply_scan(found(&[("other", 3)]));
        }
        assert_eq!(by_pipe.agents.len(), 1);
        assert_eq!(by_scan.agents.len(), 1);

        for s in [&mut by_pipe, &mut by_scan] {
            s.apply_scan(Vec::new());
        }
        let rows = |s: &State| -> Vec<AgentId> { s.agents.iter().map(|a| a.id.clone()).collect() };
        assert_eq!(rows(&by_pipe), rows(&by_scan), "both panels agree");
        assert!(by_pipe.agents.is_empty(), "the vanished agent is gone from both");
    }

    /// The bug: a hook-reported foreign row outlived its process forever,
    /// because only the agent's own panel ever saw the `ended`.
    #[test]
    fn a_scan_culls_a_hook_reported_foreign_row_whose_process_is_gone() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "3"), ("session", "other"), ("status", "working")]));
        s.apply_scan(found(&[("other", 3)]));
        assert_eq!(s.agents.len(), 1);

        assert!(s.apply_scan(Vec::new()), "the foreign row drops");
        assert!(s.agents.is_empty());
    }

    /// The existing protection, which the fix must not regress: the hook owns
    /// the home session, so a scan that raced it must not delete its row.
    #[test]
    fn a_scan_never_culls_a_hook_reported_home_row() {
        let mut s = state();
        s.apply_scan(Vec::new());
        s.handle_status(&args(&[("pane_id", "4"), ("session", "mob"), ("status", "working")]));
        assert!(!s.apply_scan(Vec::new()), "nothing changed");
        assert_eq!(s.agents.len(), 1, "the home row survives a scan that missed it");
    }

    /// A panel that has piped rows but has never completed a scan has no
    /// evidence of absence, so the first scan must not wipe them.
    #[test]
    fn the_first_scan_does_not_cull_foreign_rows() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "3"), ("session", "other"), ("status", "working")]));
        assert!(!s.apply_scan(Vec::new()), "the first scan only records that it ran");
        assert_eq!(s.agents.len(), 1);

        assert!(s.apply_scan(Vec::new()), "the second one culls");
        assert!(s.agents.is_empty());
    }

    /// A failed `ps` produces no output, which is indistinguishable from "no
    /// agents anywhere". Applying it would wipe every foreign row, so the
    /// nonzero exit must be dropped before it reaches `apply_scan`.
    #[test]
    fn a_failed_scan_never_culls() {
        use zellij_tile::prelude::ZellijPlugin;

        let mut s = state();
        s.handle_status(&args(&[("pane_id", "3"), ("session", "other"), ("status", "working")]));
        s.apply_scan(Vec::new());
        assert_eq!(s.agents.len(), 1);

        let mut ctx = BTreeMap::new();
        ctx.insert(
            crate::install::CTX_KEY.to_string(),
            crate::discover::CTX_SCAN.to_string(),
        );
        s.update(Event::RunCommandResult(
            Some(1),
            Vec::new(),
            b"ps: command not found".to_vec(),
            ctx,
        ));
        assert_eq!(s.agents.len(), 1, "a failed scan leaves the list alone");
    }

    /// A dead session's processes are gone, so the scan cannot see them. The row
    /// stays `unknown` rather than vanishing.
    #[test]
    fn a_scan_does_not_cull_a_row_whose_session_died() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "3"), ("session", "other"), ("status", "working")]));
        s.apply_scan(found(&[("other", 3)]));
        s.apply_sessions(vec!["mob".into()]);
        assert_eq!(s.agents[0].status, Status::Unknown);

        assert!(!s.apply_scan(Vec::new()));
        assert_eq!(s.agents.len(), 1, "the row persists to show the agent existed");
    }

    /// `ended` is unchanged: it still removes the row in the agent's own panel.
    #[test]
    fn ended_still_removes_the_row_locally() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "3"), ("session", "mob"), ("status", "working")]));
        assert!(s.handle_status(&args(&[("pane_id", "3"), ("session", "mob"), ("status", "ended")])));
        assert!(s.agents.is_empty());
    }

    /// A foreign row's status is frozen the moment it arrives, so past the
    /// threshold the panel stops asserting it.
    #[test]
    fn a_stale_foreign_row_decays_to_unknown() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "3"), ("session", "other"), ("status", "working")]));
        s.handle_status(&args(&[("pane_id", "4"), ("session", "mob"), ("status", "working")]));

        s.now = STALE_AFTER - TICK;
        assert!(!s.age_foreign_rows(), "not stale yet");

        s.now = STALE_AFTER;
        assert!(s.age_foreign_rows());
        let foreign = s.agents.iter().find(|a| a.session() == "other").unwrap();
        assert_eq!(foreign.status, Status::Unknown);
        let home = s.agents.iter().find(|a| a.session() == "mob").unwrap();
        assert_eq!(home.status, Status::Working, "the home row is refreshed by its hook");
    }

    /// A foreign agent that keeps heartbeating is not stale. `status_since` only
    /// moves on a change, so aging must key on the last report instead.
    #[test]
    fn a_heartbeating_foreign_row_does_not_decay() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "3"), ("session", "other"), ("status", "working")]));
        for _ in 0..3 {
            s.now += STALE_AFTER - TICK;
            s.handle_status(&args(&[("pane_id", "3"), ("session", "other"), ("status", "working")]));
            assert!(!s.age_foreign_rows(), "a fresh heartbeat resets the clock");
        }
        assert_eq!(s.agents[0].status, Status::Working);
        assert_eq!(s.agents[0].turns, 1, "heartbeats are not new turns");
    }

    /// `found` asserts nothing about state, so there is nothing to decay to and
    /// the row must keep saying `found`.
    #[test]
    fn a_discovered_foreign_row_does_not_decay() {
        let mut s = state();
        s.apply_scan(found(&[("other", 3)]));
        s.now = STALE_AFTER * 2.0;
        assert!(!s.age_foreign_rows());
        assert_eq!(s.agents[0].status, Status::Discovered);
    }

    /// The spinner's own condition is not enough: a foreign `waiting` row still
    /// needs ticks to reach its decay.
    #[test]
    fn the_timer_runs_for_a_foreign_row_that_is_not_spinning() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "3"), ("session", "other"), ("status", "waiting")]));
        s.timer_running = false;
        s.arm_timer();
        assert!(s.timer_running, "a foreign row must keep the clock running");

        let mut s = state();
        s.handle_status(&args(&[("pane_id", "3"), ("session", "mob"), ("status", "waiting")]));
        s.timer_running = false;
        s.arm_timer();
        assert!(!s.timer_running, "a home row needs no clock: its hook refreshes it");
    }

    /// The widened `arm_timer` must not leave a panel ticking forever: once a
    /// foreign row has decayed there is nothing left for the clock to do.
    #[test]
    fn the_clock_stops_once_foreign_rows_have_decayed() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "3"), ("session", "other"), ("status", "waiting")]));
        s.timer_running = false;
        s.arm_timer();
        assert!(s.timer_running);

        s.now = STALE_AFTER;
        assert!(s.age_foreign_rows());
        s.timer_running = false;
        s.arm_timer();
        assert!(!s.timer_running, "no permanent wakeup");
    }

    fn spool(pairs: &[(&str, &str)]) -> crate::discover::Spooled {
        let args: BTreeMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        crate::discover::Spooled {
            session: args.get("session").cloned().unwrap_or_default(),
            pane_id: args.get("pane_id").and_then(|p| p.parse().ok()).unwrap_or(0),
            ts: args.get("ts").and_then(|t| t.parse().ok()).unwrap_or(100.0),
            args,
        }
    }

    fn scan_with(found: Vec<crate::discover::Found>, spooled: Vec<crate::discover::Spooled>) -> crate::discover::Scan {
        crate::discover::Scan {
            found,
            spooled,
            complete: true,
        }
    }

    /// The headline: a foreign agent's status arrives without a panel in its
    /// session, which the pipe transport cannot do.
    #[test]
    fn a_spool_record_gives_a_foreign_row_live_status() {
        let mut s = state();
        s.apply_scan(found(&[("other", 3)]));
        assert_eq!(s.agents[0].status, Status::Discovered);

        assert!(s.apply_scan_result(scan_with(
            found(&[("other", 3)]),
            vec![spool(&[
                ("ts", "100"),
                ("pane_id", "3"),
                ("session", "other"),
                ("status", "waiting"),
                ("task", "Fix the parser"),
                ("detail", "needs approval: rm -rf"),
                ("cwd", "/Users/x/Projects/web"),
            ])],
        )));
        let a = &s.agents[0];
        assert_eq!(a.status, Status::Waiting);
        assert_eq!(a.task.as_deref(), Some("Fix the parser"));
        assert_eq!(a.detail.as_deref(), Some("needs approval: rm -rf"));
        assert_eq!(a.project(), "web");
    }

    /// The core safety rule: existence comes from the process scan, never from a
    /// file. Otherwise a leftover record resurrects an agent that exited.
    #[test]
    fn a_spool_record_never_creates_a_row() {
        let mut s = state();
        assert!(!s.apply_scan_result(scan_with(
            Vec::new(),
            vec![spool(&[
                ("ts", "100"),
                ("pane_id", "3"),
                ("session", "other"),
                ("status", "working"),
            ])],
        )));
        assert!(s.agents.is_empty(), "no process, no row");
    }

    /// A killed agent leaves its file behind; the scan is what retires the row.
    #[test]
    fn a_stale_file_cannot_keep_a_dead_agent_alive() {
        let mut s = state();
        s.apply_scan(found(&[("other", 3)]));
        let rec = spool(&[
            ("ts", "100"),
            ("pane_id", "3"),
            ("session", "other"),
            ("status", "working"),
        ]);
        s.apply_scan_result(scan_with(found(&[("other", 3)]), vec![rec]));
        assert_eq!(s.agents.len(), 1);

        let rec = spool(&[
            ("ts", "100"),
            ("pane_id", "3"),
            ("session", "other"),
            ("status", "working"),
        ]);
        assert!(s.apply_scan_result(scan_with(Vec::new(), vec![rec])));
        assert!(s.agents.is_empty(), "the process is gone, so the row goes");
    }

    /// The pipe reaches this session directly and is both fresher and
    /// authoritative; a spool read must not fight it.
    #[test]
    fn the_spool_never_overwrites_a_home_row() {
        let mut s = state();
        s.handle_status(&args(&[
            ("pane_id", "3"),
            ("session", "mob"),
            ("status", "working"),
            ("task", "from the pipe"),
        ]));
        s.apply_scan_result(scan_with(
            found(&[("mob", 3)]),
            vec![spool(&[
                ("ts", "999"),
                ("pane_id", "3"),
                ("session", "mob"),
                ("status", "failed"),
                ("task", "from the spool"),
            ])],
        ));
        assert_eq!(s.agents[0].status, Status::Working, "the pipe owns home rows");
        assert_eq!(s.agents[0].task.as_deref(), Some("from the pipe"));
    }

    /// Pane ids are recycled. A record from last week's agent on this pane must
    /// not colour today's - the subtlest failure here, because the data looks
    /// plausible rather than corrupt.
    #[test]
    fn a_recycled_pane_id_does_not_inherit_the_old_agents_status() {
        let mut s = state();
        s.handle_status(&args(&[
            ("pane_id", "3"),
            ("session", "other"),
            ("status", "idle"),
            ("session_id", "new-uuid"),
        ]));
        assert!(!s.apply_scan_result(scan_with(
            found(&[("other", 3)]),
            vec![spool(&[
                ("ts", "100"),
                ("pane_id", "3"),
                ("session", "other"),
                ("session_id", "old-uuid"),
                ("status", "failed"),
                ("task", "last week's work"),
            ])],
        )));
        assert_eq!(s.agents[0].status, Status::Idle, "the old record is ignored");
        assert!(s.agents[0].task.is_none());
    }

    /// A week-old file must never render, even if its row still exists.
    #[test]
    fn a_record_older_than_the_stale_threshold_is_ignored() {
        let mut s = state();
        s.apply_scan(found(&[("other", 3)]));
        s.apply_scan_result(scan_with(
            found(&[("other", 3)]),
            vec![
                spool(&[
                    ("ts", "100000"),
                    ("pane_id", "4"),
                    ("session", "other"),
                    ("status", "working"),
                ]),
                spool(&[
                    ("ts", "100"),
                    ("pane_id", "3"),
                    ("session", "other"),
                    ("status", "failed"),
                ]),
            ],
        ));
        assert_eq!(
            s.agents.iter().find(|a| a.pane_id() == 3).unwrap().status,
            Status::Discovered,
            "an ancient record must not be applied"
        );
    }

    /// Records are dated against each other, so a host clock jump cannot pin a
    /// row as permanently current.
    #[test]
    fn a_future_timestamp_does_not_make_a_row_immortal() {
        let mut s = state();
        s.apply_scan(found(&[("other", 3)]));
        s.apply_scan_result(scan_with(
            found(&[("other", 3)]),
            vec![spool(&[
                ("ts", "99999999"),
                ("pane_id", "3"),
                ("session", "other"),
                ("status", "working"),
            ])],
        ));
        assert_eq!(s.agents[0].status, Status::Working);
        s.apply_scan_result(scan_with(
            found(&[("other", 3)]),
            vec![spool(&[
                ("ts", "100"),
                ("pane_id", "3"),
                ("session", "other"),
                ("status", "failed"),
            ])],
        ));
        assert_eq!(
            s.agents[0].status,
            Status::Working,
            "the far-older record is stale relative to the newest seen"
        );
    }

    /// An empty or absent spool is the normal first-run state and must render
    /// exactly as before the feature existed.
    #[test]
    fn an_empty_spool_leaves_scan_rows_untouched() {
        let mut s = state();
        assert!(s.apply_scan_result(scan_with(found(&[("other", 3)]), Vec::new())));
        assert_eq!(s.agents[0].status, Status::Discovered);
        assert!(!s.apply_scan_result(scan_with(found(&[("other", 3)]), Vec::new())));
        assert_eq!(s.agents[0].status, Status::Discovered, "still just found");
    }

    /// A truncated read is indistinguishable from "nothing is running", so it
    /// must change nothing at all.
    #[test]
    fn an_incomplete_scan_is_ignored_entirely() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "3"), ("session", "other"), ("status", "working")]));
        s.apply_scan(found(&[("other", 3)]));

        let incomplete = crate::discover::Scan {
            found: Vec::new(),
            spooled: Vec::new(),
            complete: false,
        };
        assert!(!s.apply_scan_result(incomplete));
        assert_eq!(s.agents.len(), 1, "a partial read must not cull");
    }

    /// Two panels with different histories must agree once both have seen the
    /// same spool - the convergence contract from cross-session-consistency.md.
    #[test]
    fn two_panels_converge_on_spooled_status() {
        let mut by_pipe = state();
        by_pipe.handle_status(&args(&[("pane_id", "3"), ("session", "other"), ("status", "idle")]));
        let mut by_scan = state();

        for s in [&mut by_pipe, &mut by_scan] {
            s.apply_scan_result(scan_with(
                found(&[("other", 3)]),
                vec![spool(&[
                    ("ts", "100"),
                    ("pane_id", "3"),
                    ("session", "other"),
                    ("status", "waiting"),
                    ("task", "same task"),
                ])],
            ));
        }
        let view = |s: &State| -> Vec<(Status, Option<String>)> {
            s.agents.iter().map(|a| (a.status, a.task.clone())).collect()
        };
        assert_eq!(view(&by_pipe), view(&by_scan));
        assert_eq!(by_pipe.agents[0].status, Status::Waiting);
    }

    /// A spool read refreshes `last_report`, so a heartbeating foreign agent
    /// must not decay to `unknown` while it is demonstrably alive.
    #[test]
    fn spooled_rows_do_not_decay_while_being_refreshed() {
        let mut s = state();
        s.apply_scan(found(&[("other", 3)]));
        for i in 0..3 {
            s.now += STALE_AFTER - TICK;
            s.apply_scan_result(scan_with(
                found(&[("other", 3)]),
                vec![spool(&[
                    ("ts", &(1000 + i * 1000).to_string()),
                    ("pane_id", "3"),
                    ("session", "other"),
                    ("status", "working"),
                ])],
            ));
            assert!(!s.age_foreign_rows(), "a fresh record resets the clock (round {})", i);
        }
        assert_eq!(s.agents[0].status, Status::Working);
    }

    /// Counter fields are deltas and meaningless when replayed from a snapshot;
    /// applying them on every poll would inflate the count without bound.
    #[test]
    fn re_reading_the_same_record_does_not_accumulate() {
        let mut s = state();
        s.apply_scan(found(&[("other", 3)]));
        let rec = || {
            spool(&[
                ("ts", "100"),
                ("pane_id", "3"),
                ("session", "other"),
                ("status", "working"),
                ("subagent_delta", "1"),
                ("task_delta", "1"),
            ])
        };
        s.apply_scan_result(scan_with(found(&[("other", 3)]), vec![rec()]));
        let first = (s.agents[0].subagents, s.agents[0].tasks_total);
        for _ in 0..5 {
            s.apply_scan_result(scan_with(found(&[("other", 3)]), vec![rec()]));
        }
        assert_eq!(
            (s.agents[0].subagents, s.agents[0].tasks_total),
            first,
            "a re-read snapshot must be idempotent"
        );
    }

    /// The whole loop, end to end: the real hook script writes a real spool,
    /// the real scan script reads it, and the real parser and merge turn it
    /// into a live foreign row. Every layer in between is exercised, which no
    /// single-layer test does.
    #[cfg(unix)]
    #[test]
    fn the_real_hook_and_scan_produce_a_live_foreign_row() {
        use std::process::Command;

        let root = env!("CARGO_MANIFEST_DIR");
        let hook = format!("{}/scripts/zj-agent-mob-hook.sh", root);
        if Command::new("jq").arg("--version").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("zj-loop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (bin, spool) = (dir.join("bin"), dir.join("spool"));
        std::fs::create_dir_all(&bin).unwrap();
        let zellij = bin.join("zellij");
        std::fs::write(&zellij, "#!/bin/sh\nexit 0\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&zellij, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());

        for ev in [
            r#"{"hook_event_name":"UserPromptSubmit","session_id":"uuid-xyz","cwd":"/Users/x/Projects/web"}"#,
            r#"{"hook_event_name":"Notification","type":"permission_prompt","message":"needs permission"}"#,
        ] {
            let out = Command::new("sh")
                .arg(&hook)
                .env("PATH", &path)
                .env("ZELLIJ_PANE_ID", "7")
                .env("ZELLIJ_SESSION_NAME", "other")
                .env("ZJ_AGENT_SPOOL_DIR", &spool)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .spawn()
                .and_then(|mut c| {
                    use std::io::Write;
                    c.stdin.take().unwrap().write_all(ev.as_bytes())?;
                    c.wait()
                });
            assert!(out.is_ok_and(|s| s.success()), "the hook must always exit 0");
        }

        let script = crate::discover::scan_script(&["claude", "codex"]);
        let out = Command::new("sh")
            .arg("-c")
            .arg(&script)
            .env("ZJ_AGENT_SPOOL_DIR", &spool)
            .output()
            .expect("scan runs");
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let scan = crate::discover::parse(&stdout);
        assert!(scan.complete, "sentinel missing: {:?}", stdout);
        assert_eq!(scan.spooled.len(), 1, "one record: {:?}", stdout);

        let mut s = state();
        // The process scan is what justifies the row; the spool only refines it.
        s.apply_scan(found(&[("other", 7)]));
        assert_eq!(s.agents[0].status, Status::Discovered);

        assert!(s.apply_scan_result(crate::discover::Scan {
            found: found(&[("other", 7)]),
            spooled: scan.spooled,
            complete: true,
        }));
        let a = &s.agents[0];
        assert_eq!(a.status, Status::Waiting, "live status without a panel in its session");
        assert_eq!(a.detail.as_deref(), Some("needs permission"));
        // Carried forward by the hook: Notification's payload has neither.
        assert_eq!(a.session_id, "uuid-xyz");
        assert_eq!(a.project(), "web");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Real bytes captured from this machine: two agents, two live sessions.
    #[test]
    fn real_machine_capture_renders_live_cross_session_rows() {
        use crate::agent::RowCtx;
        use crate::util::testing::item_text;
        let raw = "SCAN dotfiles-split 2 claude\nSCAN zj-agent-mob 17 claude\nSPOOL /var/folders/cl/kcw___6n0dz_bsmzz_hmxf3m0000gn/T//zj-live-verify/status/dotfiles-split.2:ts=1786299306,pane_id=2,session=dotfiles-split,tool=claude,status=waiting,session_id=,cwd=,task=,detail=needs approval: git push --force,perm_mode=,agent_type=\nSPOOL /var/folders/cl/kcw___6n0dz_bsmzz_hmxf3m0000gn/T//zj-live-verify/status/zj-agent-mob.17:ts=1786299296,pane_id=17,session=zj-agent-mob,tool=claude,status=working,session_id=live-uuid,cwd=/Users/momo/Projects/zj-agent-mob,task=,detail=,perm_mode=,agent_type=\nSCANEND\n";
        let scan = crate::discover::parse(raw);
        assert!(scan.complete);
        assert_eq!(scan.found.len(), 2, "two real agents");
        assert_eq!(scan.spooled.len(), 2, "two real records");

        let mut s = State {
            permissions_granted: true,
            popup_on_waiting: false,
            session_name: "zj-agent-mob".into(),
            live_sessions: vec!["zj-agent-mob".into(), "dotfiles-split".into()],
            discover: true,
            ..Default::default()
        };
        s.apply_scan_result(scan);
        let rows: Vec<String> = s
            .agents
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let icon = s.icon_for(a);
                item_text(&a.list_item(
                    i,
                    RowCtx {
                        selected: false,
                        icon,
                        now: s.now,
                        cols: 110,
                        show_cwd: true,
                        home: &s.session_name,
                    },
                ))
            })
            .collect();
        let all = rows.join("\n");
        assert!(all.contains("waiting"), "the foreign agent is blocked: {}", all);
        // The session column is 10 wide, so a longer name is truncated.
        assert!(all.contains("dotfiles-"), "foreign row names its session: {}", all);
        let foreign = s.agents.iter().find(|a| a.session() == "dotfiles-split").unwrap();
        assert_eq!(foreign.status, Status::Waiting);
        assert_eq!(foreign.detail.as_deref(), Some("needs approval: git push --force"));
        assert!(rows[0].contains("waiting"), "the blocked agent sorts first: {:?}", rows);
        println!("{}", all);
    }

    #[test]
    fn ask_lookup_is_session_scoped() {
        let mut s = state();
        s.handle_ask(&args(&[
            ("pane_id", "3"),
            ("session", "other"),
            ("verdict_file", "/tmp/v"),
        ]));
        let home = AgentId {
            session: "mob".into(),
            pane_id: 3,
        };
        let foreign = AgentId {
            session: "other".into(),
            pane_id: 3,
        };
        assert!(s.ask_for(&home).is_none());
        assert!(s.ask_for(&foreign).is_some());
    }
}
