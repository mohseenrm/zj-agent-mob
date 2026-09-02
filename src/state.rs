//! Panel state: pipe handling, pane reconciliation.

use std::collections::BTreeMap;
use zellij_tile::prelude::*;

use crate::agent::{Agent, AgentId, Block};
use crate::host;
use crate::install::Install;
use crate::status::Status;
use crate::{SIGINT_BYTE, SPINNER, STALE_AFTER, TICK};

/// A permission prompt parked by a blocked hook, waiting on a verdict.
pub(crate) struct Ask {
    pub(crate) id: AgentId,
    pub(crate) verdict_file: String,
    pub(crate) tool_name: String,
    pub(crate) tool_arg: String,
}

/// How the list is ordered. Urgency alone scatters one project across the
/// screen once there are more agents than rows; grouping trades that for a
/// header per project, urgency still deciding order within each group.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub(crate) enum Grouping {
    #[default]
    Urgency,
    Project,
    Session,
}

impl Grouping {
    pub(crate) fn next(self) -> Self {
        match self {
            Grouping::Urgency => Grouping::Project,
            Grouping::Project => Grouping::Session,
            Grouping::Session => Grouping::Urgency,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Grouping::Urgency => "urgency",
            Grouping::Project => "project",
            Grouping::Session => "session",
        }
    }
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
    /// Sanitized name -> the real one Zellij knows it by.
    ///
    /// Identity is sanitized so it can key a spool filename, but `zellij
    /// --session` needs the name the user actually gave. A session called
    /// "my session" is stored as `my_session` and addressing that fails, so the
    /// two uses must not share a string.
    pub(crate) session_names: BTreeMap<String, String>,
    /// A scan is in flight, so a second one would be wasted work.
    pub(crate) scan_pending: bool,
    /// `now` at the last scan dispatch, which paces the spool poll. Stamped on
    /// dispatch rather than on the result, so a scan that never returns cannot
    /// wedge the poll; `scan_pending` is what prevents overlap.
    pub(crate) last_scan_at: f64,
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
    /// `now` when `spool_epoch` last advanced, so a frozen epoch still ages.
    pub(crate) spool_epoch_at: f64,
    pub(crate) notifier: crate::notify::Notifier,
    /// The last summary published, so an unchanged fleet is not republished on
    /// every tick.
    pub(crate) last_summary: String,
    pub(crate) summary_path: String,
    /// A free-text reply being typed for the selected agent.
    pub(crate) reply: Option<Reply>,
    /// Digits typed after `G`, awaiting Enter or a non-digit. Vim-style, so a
    /// row past 9 is still reachable without stealing a letter command.
    pub(crate) jump_buf: Option<String>,
    /// The `/` prompt, while it is open. `Some` narrows the list to matches and
    /// turns every printable key into query text.
    pub(crate) find: Option<Find>,
    /// First visible row, kept so the selection stays on screen.
    pub(crate) scroll: usize,
    /// The last cross-session action that failed. Those run through the `zellij`
    /// binary, so a failure is otherwise invisible: the row would vanish while
    /// the agent kept running.
    pub(crate) action_error: Option<String>,
    pub(crate) grouping: Grouping,
}

/// A reply being composed in the panel, bound to the agent it will be sent to.
/// Holding the id rather than an index means a re-sort mid-typing cannot
/// redirect the text at another agent.
pub(crate) struct Reply {
    pub(crate) id: AgentId,
    pub(crate) text: String,
}

/// A fuzzy search being typed at the `/` prompt. The cursor is an id, not an
/// index: pipes arrive and re-sort the list mid-typing, and an index would then
/// point Enter at a stranger. `None` means "the best match".
#[derive(Default)]
pub(crate) struct Find {
    pub(crate) query: String,
    pub(crate) cursor: Option<AgentId>,
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
        // Foreign rows need ticks whatever their status: the clock is what
        // paces the spool poll, and an `unknown` row is precisely the one the
        // poll exists to recover. A row whose session is gone is excluded -
        // nothing can refresh it, so it would hold the clock open forever.
        // A blocked row needs ticks too: it animates nothing, so without this
        // the clock stops and it can never reach the escalation threshold.
        let (home, now) = (&self.session_name, self.now);
        let needed = self
            .agents
            .iter()
            .any(|a| a.status.is_active() || (a.id.session != *home && a.session_alive) || a.escalation_pending(now));
        if needed {
            self.timer_running = true;
            host::set_timeout(TICK);
        }
    }

    /// Probes for a notifier binary once, after permissions land. Detection is
    /// cached rather than repeated per notification.
    pub(crate) fn detect_notifier(&mut self) {
        if !self.permissions_granted || self.notifier.triggers.is_empty() || !self.notifier.binary.is_empty() {
            return;
        }
        let mut ctx = BTreeMap::new();
        ctx.insert(
            crate::install::CTX_KEY.to_string(),
            crate::notify::CTX_DETECT.to_string(),
        );
        host::run_command(&["sh", "-c", crate::notify::detect_script()], ctx);
    }

    /// Ticks regardless of whether any row is animating. A pending notification
    /// or an escalating `waiting` row needs the clock to advance even when
    /// every agent is sitting still.
    pub(crate) fn force_timer(&mut self) {
        if !self.timer_running {
            self.timer_running = true;
            host::set_timeout(TICK);
        }
    }

    /// The one-line fleet summary status bars render. Published only when it
    /// changes, so an idle fleet costs nothing.
    pub(crate) fn summary_line(&self) -> String {
        let (failed, waiting, working, done) = self.counts();
        let mut parts = Vec::new();
        for (n, label) in [
            (failed, "failed"),
            (waiting, "waiting"),
            (working, "working"),
            (done, "done"),
        ] {
            if n > 0 {
                parts.push(format!("{} {}", n, label));
            }
        }
        parts.join(" \u{b7} ")
    }

    /// The same counts as `summary_line`, as `k=v` pairs a consumer can read
    /// without parsing prose. Always every key, including zeros: a consumer
    /// testing `waiting>0` should not have to distinguish absent from none.
    pub(crate) fn summary_kv(&self) -> String {
        let (failed, waiting, working, done) = self.counts();
        format!(
            "failed={} waiting={} working={} done={} found={} total={}",
            failed,
            waiting,
            working,
            done,
            self.discovered_count(),
            self.agents.len()
        )
    }

    /// Pushes the summary out to any status bar listening, when it has moved.
    pub(crate) fn publish_summary(&mut self) {
        if self.summary_path.is_empty() {
            return;
        }
        let line = self.summary_line();
        if line == self.last_summary {
            return;
        }
        self.last_summary = line.clone();
        host::publish_summary(&line, &self.summary_path, &self.summary_kv());
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

    /// The name Zellij knows a session by, which is what `--session` needs.
    /// Falls back to the sanitized form when the session is not listed: it is
    /// the best guess available, and is correct for every name that survives
    /// sanitizing unchanged.
    pub(crate) fn real_session(&self, sanitized: &str) -> String {
        self.session_names
            .get(sanitized)
            .cloned()
            .unwrap_or_else(|| sanitized.to_string())
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

    /// One clock tick. Lives here rather than in the `Timer` arm so the poll it
    /// drives is testable: `host` calls no-op off-wasm, so a dispatch is only
    /// observable through `scan_pending` / `last_scan_at`.
    pub(crate) fn on_tick(&mut self) -> bool {
        self.timer_running = false;
        self.frame = self.frame.wrapping_add(1);
        self.now += TICK;
        if self.spool_poll_due() {
            self.request_scan();
        }
        let aged = self.age_foreign_rows();
        self.notifier.flush(self.now);
        self.publish_summary();
        // A pending flush keeps the clock running on its own: the window must
        // close even when no row is animating.
        if self.notifier.flush_at.is_some() {
            self.force_timer();
        } else {
            self.arm_timer();
        }
        aged || self.agents.iter().any(|a| a.status.is_active())
    }

    /// A foreign row's status is a snapshot. Past `STALE_AFTER` with nothing
    /// refreshing it, the panel says `unknown` rather than keeping a `working`
    /// it can no longer vouch for. The spool poll is what refreshes it.
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
            // Text aimed at an agent that just exited must not survive to be
            // sent to whichever row inherits the selection.
            if self.reply.as_ref().is_some_and(|r| r.id == id) {
                self.reply = None;
            }
            self.clamp_selection();
            let removed = self.agents.len() != before;
            if removed {
                self.publish_summary();
            }
            return removed;
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
        let transitioned;
        if let Some(agent) = self.agents.iter_mut().find(|a| a.id == id) {
            let changed = agent.status != status;
            newly_waiting = changed && status == Status::Waiting;
            transitioned = changed;
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
            // Only a blocked state has a reason to be blocked. Clearing it on
            // everything else stops a stale `plan` label outliving its prompt.
            agent.block = match status {
                Status::Waiting | Status::IdleWait => args.get("block").and_then(|b| Block::parse(b)).or(agent.block),
                _ => None,
            };
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
            transitioned = true;
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
                notified: false,
                block: args.get("block").and_then(|b| Block::parse(b)),
            });
        }

        if transitioned {
            self.queue_notification(&id, status);
        }
        self.sort_agents();
        self.publish_summary();
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

    /// Queues a desktop notification for a transition, and arms the tick so the
    /// coalescing window can close even when nothing else is animating.
    fn queue_notification(&mut self, id: &AgentId, status: Status) {
        let task = self
            .agents
            .iter()
            .find(|a| &a.id == id)
            .map(|a| a.display_task().to_string())
            .unwrap_or_default();
        if self.notifier.queue(id, status, &task, self.now) {
            if let Some(a) = self.agents.iter_mut().find(|a| &a.id == id) {
                a.notified = true;
            }
            self.force_timer();
        }
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

    /// Interrupts an agent, in this session or another. The plugin's own shim
    /// is session-local, so a foreign pane is reached through the `zellij`
    /// binary, which does take a session.
    pub(crate) fn interrupt_pane(&self, id: &AgentId, foreign: bool) {
        match foreign {
            true => host::session_action(
                &self.real_session(&id.session),
                &["write", &SIGINT_BYTE.to_string(), "--pane-id", &id.pane_id.to_string()],
                "kill",
            ),
            false => host::send_sigint_to_pane_id(PaneId::Terminal(id.pane_id)),
        }
    }

    pub(crate) fn close_pane(&self, id: &AgentId, foreign: bool) {
        match foreign {
            true => host::session_action(
                &self.real_session(&id.session),
                &["close-pane", "--pane-id", &id.pane_id.to_string()],
                "kill",
            ),
            false => host::close_terminal_pane(id.pane_id),
        }
    }

    /// Clears every `done` badge at once. With a fleet running, acknowledging
    /// six finished agents one keypress at a time is its own chore.
    pub(crate) fn dismiss_all_done(&mut self) -> bool {
        let now = self.now;
        let mut changed = false;
        for agent in self.agents.iter_mut() {
            if agent.status == Status::Done {
                agent.status = Status::Idle;
                agent.status_since = now;
                changed = true;
            }
        }
        if changed {
            self.sort_agents();
            self.publish_summary();
        }
        changed
    }

    /// Whether the selected agent can be typed into. Only a blocked agent: any
    /// other state has no prompt waiting, so the keystrokes would land mid-turn
    /// as stray input. A foreign row is reachable through the CLI.
    pub(crate) fn can_reply_selected(&self) -> bool {
        self.agents
            .get(self.selected)
            .map(|a| matches!(a.status, Status::Waiting | Status::IdleWait) && a.session_alive)
            .unwrap_or(false)
    }

    /// Opens the one-line editor for a free-text reply.
    pub(crate) fn begin_reply(&mut self) -> bool {
        if !self.can_reply_selected() {
            return false;
        }
        let Some(id) = self.agents.get(self.selected).map(|a| a.id.clone()) else {
            return false;
        };
        self.reply = Some(Reply {
            id,
            text: String::new(),
        });
        true
    }

    /// Types into an agent's pane. The agent is left `working`: it has been
    /// answered, and waiting for the next heartbeat to say so would leave a
    /// stale `waiting` on screen.
    ///
    /// A composed reply is sent to the agent it was written for, not to whatever
    /// the cursor holds now: another agent blocking mid-typing re-sorts the list
    /// under the selection, and that must not redirect the text.
    pub(crate) fn send_reply(&mut self, text: &str) -> bool {
        let id = match self.reply.as_ref().map(|r| r.id.clone()) {
            Some(id) => id,
            None => match self.can_reply_selected() {
                true => match self.agents.get(self.selected).map(|a| a.id.clone()) {
                    Some(id) => id,
                    None => return false,
                },
                false => return false,
            },
        };
        // The bound agent must still be answerable; it may have moved on or
        // exited while the reply was being typed.
        let sendable = self
            .agents
            .iter()
            .any(|a| a.id == id && matches!(a.status, Status::Waiting | Status::IdleWait) && a.session_alive);
        if !sendable {
            self.reply = None;
            return false;
        }
        let foreign = !self.session_name.is_empty() && id.session != self.session_name;
        match foreign {
            true => host::session_action(
                &self.real_session(&id.session),
                &["write-chars", "--pane-id", &id.pane_id.to_string(), text],
                "reply",
            ),
            false => host::write_chars_to_pane_id(text, PaneId::Terminal(id.pane_id)),
        }
        self.reply = None;
        self.asks.retain(|a| a.id != id);
        let now = self.now;
        if let Some(agent) = self.agents.iter_mut().find(|a| a.id == id) {
            agent.status = Status::Working;
            agent.status_since = now;
            agent.detail = Some("replied from panel".to_string());
        }
        self.sort_agents();
        self.publish_summary();
        self.arm_timer();
        true
    }

    /// Opens a floating pane running an agent in the selected row's directory,
    /// or the panel's own if there is no row to borrow one from.
    pub(crate) fn spawn_agent(&mut self) -> bool {
        let cwd = self
            .agents
            .get(self.selected)
            .map(|a| a.cwd.clone())
            .filter(|c| !c.is_empty());
        let tool = self
            .agents
            .get(self.selected)
            .map(|a| a.tool.clone())
            .filter(|t| t == "claude" || t == "codex")
            .unwrap_or_else(|| "claude".to_string());
        host::open_command_pane_floating(
            CommandToRun {
                path: tool.into(),
                args: Vec::new(),
                cwd: cwd.map(Into::into),
            },
            None,
            BTreeMap::new(),
        );
        // The pane opens as a command pane, so a missing binary surfaces in that
        // pane with its own exit status rather than vanishing. Hiding the panel
        // is what makes it visible.
        self.hidden = true;
        host::hide_self();
        true
    }

    /// Whether the spool is due a re-read.
    ///
    /// The spool is the only thing that refreshes a foreign row's status, and
    /// nothing else polls it: `request_scan` is otherwise driven by pane and
    /// session events, which do not fire while an agent is merely working. So
    /// without this a foreign row decays to `unknown` and never recovers.
    ///
    /// Gated on a foreign row existing, so a single-session panel pays nothing.
    pub(crate) fn spool_poll_due(&self) -> bool {
        if self.scan_pending || self.now - self.last_scan_at < crate::SPOOL_POLL_INTERVAL {
            return false;
        }
        let home = &self.session_name;
        self.agents.iter().any(|a| a.id.session != *home && a.session_alive)
    }

    /// Runs a scan unless one is already in flight, or discovery is switched off.
    pub(crate) fn request_scan(&mut self) {
        if self.scan_pending || !self.permissions_granted || !self.discover {
            return;
        }
        self.scan_pending = true;
        self.last_scan_at = self.now;
        crate::discover::dispatch();
        // Refreshed alongside the scan so hooks elsewhere keep fanning urgent
        // transitions to this panel while it is open.
        let real = self.real_session(&self.session_name);
        crate::discover::announce_panel(&self.session_name, &real);
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
                self.spool_epoch_at = self.now;
            }
        }
        let mut changed = self.merge_found(scan.found);
        changed |= self.apply_spool(scan.spooled);
        if changed {
            self.clamp_selection();
            self.sort_agents();
            self.publish_summary();
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
                notified: false,
                block: None,
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
        let mut transitions: Vec<(AgentId, Status, String)> = Vec::new();
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
            let Some(status) = rec.args.get("status").and_then(|s| Status::parse(s)) else {
                continue;
            };
            let age = self.spool_age(rec.ts);
            let agent = &mut self.agents[idx];

            // A blocked or idle agent writes nothing while it sits there, so
            // its record stops advancing and eventually ages past STALE_AFTER.
            // Re-reading it is still evidence: the process scan says the agent
            // is alive, and silence is exactly what these states predict. So a
            // re-read re-confirms a status the row already holds - it can never
            // change one. Without this a foreign `waiting` row decays to
            // `unknown` while the agent is blocked on you, which is the one row
            // the panel exists to show.
            let reconfirms = agent.status == status && status.persists_while_quiet();
            if age >= STALE_AFTER && !reconfirms {
                continue;
            }
            let seen_at = match reconfirms {
                true => now,
                false => now - age,
            };
            // Records are compared in their own epoch units; `last_report` is on
            // the panel's tick clock and the two are not comparable.
            if rec.ts <= agent.spool_ts {
                if reconfirms {
                    agent.last_report = seen_at;
                }
                continue;
            }
            agent.spool_ts = rec.ts;
            let mut transitioned = false;
            if agent.status != status {
                agent.status = status;
                agent.status_since = seen_at;
                changed = true;
                transitioned = true;
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
            let block = match agent.status {
                Status::Waiting | Status::IdleWait => {
                    rec.args.get("block").and_then(|b| Block::parse(b)).or(agent.block)
                }
                _ => None,
            };
            if agent.block != block {
                agent.block = block;
                changed = true;
            }
            if !rec_sid.is_empty() && agent.session_id != rec_sid {
                agent.session_id = rec_sid.to_string();
            }
            agent.last_report = seen_at;
            if transitioned {
                let task = agent.display_task().to_string();
                transitions.push((id, status, task));
            }
        }
        // Queued after the loop: `agent` borrows `self.agents` mutably, and the
        // notifier lives on `self`.
        for (id, status, task) in transitions {
            if self.notifier.queue(&id, status, &task, now) {
                if let Some(a) = self.agents.iter_mut().find(|a| a.id == id) {
                    a.notified = true;
                }
                self.force_timer();
            }
        }
        changed
    }

    /// Age of a spool record, in the panel's own tick units.
    ///
    /// The panel has no clock: `now` counts ticks since load. So a record is
    /// dated as how far it sat behind the newest record in its batch, plus how
    /// long ago that batch arrived. Anchoring to `now` is what makes a fleet
    /// that has gone entirely quiet age out: without it `spool_epoch` freezes
    /// and the last record read as current forever, fighting the tick-clock
    /// decay in `age_foreign_rows`.
    fn spool_age(&self, ts: f64) -> f64 {
        let behind = (self.spool_epoch.max(ts) - ts).max(0.0);
        behind + (self.now - self.spool_epoch_at).max(0.0)
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
        let grouping = self.grouping;
        // The group's own rank is its most urgent member, so grouping never
        // buries a blocked agent under a quiet project that sorts earlier.
        let mut best: BTreeMap<String, u8> = BTreeMap::new();
        if grouping != Grouping::Urgency {
            for a in self.agents.iter() {
                let k = a.group_key(grouping).to_string();
                let r = a.status.rank();
                best.entry(k).and_modify(|e| *e = (*e).min(r)).or_insert(r);
            }
        }
        self.agents.sort_by(|a, b| match grouping {
            Grouping::Urgency => a.status.rank().cmp(&b.status.rank()).then(a.id.cmp(&b.id)),
            _ => {
                let (ka, kb) = (a.group_key(grouping), b.group_key(grouping));
                let (ra, rb) = (
                    best.get(ka).copied().unwrap_or(u8::MAX),
                    best.get(kb).copied().unwrap_or(u8::MAX),
                );
                ra.cmp(&rb)
                    .then_with(|| ka.cmp(kb))
                    .then(a.status.rank().cmp(&b.status.rank()))
                    .then(a.id.cmp(&b.id))
            }
        });
        self.clamp_selection();
        let agents = &self.agents;
        self.notifier.retain_known(|id| agents.iter().any(|a| &a.id == id));
        // A reply loses its target when that agent's row goes away.
        if let Some(r) = &self.reply {
            let id = r.id.clone();
            if !self.agents.iter().any(|a| a.id == id) {
                self.reply = None;
            }
        }
    }

    /// Clears the notified gutter. Called when the panel becomes visible: the
    /// marks exist to survive the trip back from a banner, not past that.
    pub(crate) fn clear_notified(&mut self) -> bool {
        let mut changed = false;
        for a in self.agents.iter_mut() {
            if a.notified {
                a.notified = false;
                changed = true;
            }
        }
        changed
    }

    /// Indices into `agents` that match the open find query, best score first,
    /// ties in list order. With no prompt open, the whole list in order.
    /// Recomputed on demand rather than cached: pipes mutate `agents` freely
    /// while the prompt is open, and a stale index list would dangle.
    pub(crate) fn find_matches(&self) -> Vec<usize> {
        let Some(f) = self.find.as_ref() else {
            return (0..self.agents.len()).collect();
        };
        let mut scored: Vec<(u32, usize)> = self
            .agents
            .iter()
            .enumerate()
            .filter_map(|(i, a)| self.find_score(&f.query, a).map(|s| (s, i)))
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        scored.into_iter().map(|(_, i)| i).collect()
    }

    /// The best score across an agent's fields, weighted by how likely each is
    /// to be what the user remembers: the task and the worktree name first, the
    /// full path and session next, tool and status as a last resort.
    fn find_score(&self, query: &str, a: &Agent) -> Option<u32> {
        // The name the user knows the session by, not the sanitized file key.
        let session = self.real_session(a.session());
        let fields: [(&str, u32); 6] = [
            (a.display_task(), 4),
            (a.project(), 4),
            (&a.cwd, 2),
            (&session, 2),
            (&a.tool, 1),
            (a.status.label(), 1),
        ];
        fields
            .into_iter()
            .filter_map(|(s, w)| crate::find::score(query, s).map(|sc| sc * w))
            .max()
    }

    /// Where the find cursor sits within `matches`. An agent that vanished or
    /// stopped matching mid-search degrades to the best match rather than to a
    /// wrong-target jump.
    pub(crate) fn find_cursor_pos(&self, matches: &[usize]) -> Option<usize> {
        let f = self.find.as_ref()?;
        if matches.is_empty() {
            return None;
        }
        let held = f
            .cursor
            .as_ref()
            .and_then(|id| matches.iter().position(|&i| self.agents[i].id == *id));
        Some(held.unwrap_or(0))
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
    fn a_block_reason_is_carried_from_the_hook() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "waiting"), ("block", "plan")]));
        assert_eq!(s.agents[0].block, Some(Block::Plan));
    }

    /// A stale `plan` outliving its prompt would send you to read a pane that
    /// is no longer asking anything.
    #[test]
    fn moving_off_a_blocked_state_clears_the_reason() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "waiting"), ("block", "tool")]));
        assert_eq!(s.agents[0].block, Some(Block::Tool));
        s.handle_status(&args(&[("pane_id", "1"), ("status", "working")]));
        assert_eq!(s.agents[0].block, None, "working is not blocked on anything");
    }

    /// Heartbeats while still blocked carry no `block=`, and must not erase the
    /// reason the row already holds.
    #[test]
    fn a_report_without_a_reason_keeps_the_one_already_known() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "waiting"), ("block", "question")]));
        s.handle_status(&args(&[("pane_id", "1"), ("status", "waiting")]));
        assert_eq!(s.agents[0].block, Some(Block::Question));
    }

    #[test]
    fn an_unrecognized_reason_is_ignored_rather_than_guessed() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "waiting"), ("block", "wat")]));
        assert_eq!(s.agents[0].block, None);
    }

    #[test]
    fn idlewait_carries_its_own_reason() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "idlewait"), ("block", "idle")]));
        assert_eq!(s.agents[0].block, Some(Block::Idle));
    }

    #[test]
    fn grouping_cycles_and_labels() {
        assert_eq!(Grouping::default(), Grouping::Urgency);
        assert_eq!(Grouping::Urgency.next(), Grouping::Project);
        assert_eq!(Grouping::Project.next(), Grouping::Session);
        assert_eq!(Grouping::Session.next(), Grouping::Urgency);
        assert_eq!(Grouping::Project.label(), "project");
    }

    /// Grouping must keep every project's rows adjacent, which flat urgency
    /// order does not once two projects interleave.
    #[test]
    fn grouping_by_project_keeps_a_project_contiguous() {
        let mut s = state();
        for (pane, cwd, status) in [
            ("1", "/w/alpha", "working"),
            ("2", "/w/beta", "waiting"),
            ("3", "/w/alpha", "waiting"),
            ("4", "/w/beta", "working"),
        ] {
            s.handle_status(&args(&[("pane_id", pane), ("cwd", cwd), ("status", status)]));
        }
        s.grouping = Grouping::Project;
        s.sort_agents();
        let projects: Vec<&str> = s.agents.iter().map(|a| a.project()).collect();
        let mut runs = projects.clone();
        runs.dedup();
        assert_eq!(
            runs.len(),
            2,
            "each project must form one contiguous run, got {:?}",
            projects
        );
    }

    /// A group ranks by its most urgent member, so grouping cannot bury a
    /// blocked agent under a quiet project whose name sorts earlier.
    #[test]
    fn a_group_ranks_by_its_most_urgent_member() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("cwd", "/w/aaa"), ("status", "idle")]));
        s.handle_status(&args(&[("pane_id", "2"), ("cwd", "/w/zzz"), ("status", "waiting")]));
        s.grouping = Grouping::Project;
        s.sort_agents();
        assert_eq!(
            s.agents[0].project(),
            "zzz",
            "the blocked group must lead despite the name"
        );
    }

    #[test]
    fn urgency_grouping_is_the_plain_rank_order() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("cwd", "/w/zzz"), ("status", "idle")]));
        s.handle_status(&args(&[("pane_id", "2"), ("cwd", "/w/aaa"), ("status", "waiting")]));
        s.sort_agents();
        let labels: Vec<&str> = s.agents.iter().map(|a| a.status.label()).collect();
        assert_eq!(labels, vec!["waiting", "idle"]);
    }

    /// An agent that has never reported a cwd still needs a heading, or the
    /// group header renders empty and reads as a fault.
    #[test]
    fn an_agent_with_no_cwd_gets_a_named_bucket() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "working")]));
        assert_eq!(s.agents[0].group_key(Grouping::Project), "(no cwd)");
    }

    #[test]
    fn grouping_by_session_keys_on_the_session() {
        let mut s = state();
        s.handle_status(&args(&[
            ("pane_id", "1"),
            ("session", "other"),
            ("cwd", "/w/x"),
            ("status", "idle"),
        ]));
        assert_eq!(s.agents[0].group_key(Grouping::Session), "other");
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
    fn notifying_state() -> State {
        let mut s = state();
        s.notifier.binary = "osascript".into();
        s.notifier.cooldown = 60.0;
        s
    }

    #[test]
    fn a_transition_into_waiting_queues_a_notification() {
        let mut s = notifying_state();
        s.handle_status(&args(&[("pane_id", "3"), ("status", "working")]));
        assert!(s.notifier.pending.is_empty(), "working is not notify-worthy");
        s.handle_status(&args(&[("pane_id", "3"), ("status", "waiting")]));
        assert_eq!(s.notifier.pending.len(), 1);
    }

    /// Heartbeats repeat the same status constantly; only the edge matters.
    #[test]
    fn a_repeated_status_does_not_requeue() {
        let mut s = notifying_state();
        s.handle_status(&args(&[("pane_id", "3"), ("status", "waiting")]));
        s.notifier.pending.clear();
        s.handle_status(&args(&[("pane_id", "3"), ("status", "waiting")]));
        assert!(s.notifier.pending.is_empty(), "no transition, no notification");
    }

    /// The banner should name the work, which is what tells you whether to go.
    #[test]
    fn the_queued_notification_carries_the_task() {
        let mut s = notifying_state();
        s.handle_status(&args(&[
            ("pane_id", "3"),
            ("status", "waiting"),
            ("task", "Fix flaky checkout test"),
        ]));
        assert_eq!(s.notifier.pending[0].task, "Fix flaky checkout test");
    }

    /// A foreign agent blocking is exactly the case the panel cannot show you,
    /// so it must notify like any other.
    #[test]
    fn a_foreign_agent_notifies_too() {
        let mut s = notifying_state();
        s.handle_status(&args(&[("pane_id", "3"), ("session", "other"), ("status", "waiting")]));
        assert_eq!(s.notifier.pending.len(), 1);
        assert_eq!(s.notifier.pending[0].id.session, "other");
    }

    /// With no notifier detected the whole path must stay inert rather than
    /// queueing work that can never be sent.
    #[test]
    fn no_detected_notifier_queues_nothing() {
        let mut s = state();
        s.notifier.binary = "none".into();
        s.handle_status(&args(&[("pane_id", "3"), ("status", "waiting")]));
        assert!(s.notifier.pending.is_empty());
    }

    /// A queued notification must keep the clock running on its own: nothing
    /// else advances `now` when every agent is sitting still at `waiting`.
    #[test]
    fn queueing_arms_the_timer_with_no_active_agents() {
        let mut s = notifying_state();
        s.timer_running = false;
        s.handle_status(&args(&[("pane_id", "3"), ("status", "waiting")]));
        assert!(s.timer_running, "the flush window needs ticks to close");
    }

    /// An agent blocking in another session is precisely the case the panel
    /// cannot show you, so the spool path must notify like the pipe does.
    #[test]
    fn a_foreign_agent_blocking_via_the_spool_notifies() {
        let mut s = notifying_state();
        s.apply_scan_result(crate::discover::Scan {
            found: vec![crate::discover::Found {
                session: "other".into(),
                pane_id: 3,
                tool: "claude".into(),
            }],
            spooled: vec![crate::discover::Spooled {
                session: "other".into(),
                pane_id: 3,
                ts: 100.0,
                args: [("status", "waiting"), ("task", "Fix it"), ("session", "other")]
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            }],
            complete: true,
        });
        assert_eq!(s.notifier.pending.len(), 1, "a foreign block must reach you");
        assert_eq!(s.notifier.pending[0].id.session, "other");
        assert_eq!(s.notifier.pending[0].task, "Fix it");
    }

    /// Identity is sanitized so it can key a spool filename, but `zellij
    /// --session` needs the name the user actually gave. Addressing the
    /// sanitized form fails: the row vanishes from the panel while the agent
    /// keeps running.
    /// A cross-session kill runs through the `zellij` binary and the row is
    /// removed optimistically, so a failure has to be said out loud: otherwise
    /// the panel claims success while the agent keeps running.
    #[test]
    fn a_failed_cross_session_action_is_surfaced() {
        use zellij_tile::prelude::ZellijPlugin;
        let mut s = state();
        let ctx: BTreeMap<String, String> = [("kind", "kill")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let rendered = s.update(zellij_tile::prelude::Event::RunCommandResult(
            Some(1),
            Vec::new(),
            b"no session found".to_vec(),
            ctx,
        ));
        assert!(rendered, "a failure must trigger a re-render");
        let msg = s.action_error.as_deref().unwrap_or("");
        assert!(msg.contains("kill failed"), "{:?}", msg);
        assert!(
            msg.contains("no session found"),
            "the reason is the useful part: {:?}",
            msg
        );
    }

    /// Measured against zellij 0.44.3: addressing a session that does not exist
    /// prints to stderr and still exits 0. Trusting the exit code alone would
    /// miss exactly the failure this reporting exists for.
    #[test]
    fn a_zero_exit_with_stderr_still_counts_as_a_failure() {
        use zellij_tile::prelude::ZellijPlugin;
        let mut s = state();
        let ctx: BTreeMap<String, String> = [("kind", "kill")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        s.update(zellij_tile::prelude::Event::RunCommandResult(
            Some(0),
            b"other-session [Created 1s ago]".to_vec(),
            b"Session 'my_session' not found. The following sessions are active:".to_vec(),
            ctx,
        ));
        let msg = s.action_error.as_deref().unwrap_or("");
        assert!(
            msg.contains("kill failed"),
            "exit 0 with stderr is a failure: {:?}",
            msg
        );
    }

    /// A successful action says nothing: the row disappearing is the feedback.
    #[test]
    fn a_successful_cross_session_action_is_silent() {
        use zellij_tile::prelude::ZellijPlugin;
        let mut s = state();
        let ctx: BTreeMap<String, String> = [("kind", "reply")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        s.update(zellij_tile::prelude::Event::RunCommandResult(
            Some(0),
            Vec::new(),
            Vec::new(),
            ctx,
        ));
        assert!(s.action_error.is_none());
    }

    #[test]
    fn addressing_uses_the_real_session_name_not_the_sanitized_key() {
        let mut s = state();
        s.session_names = [("my_session".to_string(), "my session".to_string())]
            .into_iter()
            .collect();
        assert_eq!(s.real_session("my_session"), "my session");
    }

    /// A name that survives sanitizing unchanged is its own real name, and an
    /// unlisted session has nothing better to offer than the key itself.
    #[test]
    fn an_unmapped_session_falls_back_to_the_key() {
        let s = state();
        assert_eq!(s.real_session("plain-name"), "plain-name");
    }

    /// A local agent sitting at `waiting` animates nothing, so without an
    /// explicit reason to tick, the clock freezes and the row can never reach
    /// the escalation threshold. The escalation would be inert in the single-
    /// session case, which is the common one.
    #[test]
    fn a_waiting_local_row_keeps_the_clock_running() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "3"), ("status", "waiting")]));
        s.timer_running = false;
        s.arm_timer();
        assert!(
            s.timer_running,
            "a blocked row must keep ticking or it can never escalate"
        );
    }

    /// Once escalated there is nothing further to count towards, so the clock
    /// may stop again.
    #[test]
    fn an_already_escalated_row_does_not_hold_the_clock_open() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "3"), ("status", "waiting")]));
        s.now = crate::WAITING_ESCALATE_AFTER + 10.0;
        s.timer_running = false;
        s.arm_timer();
        assert!(!s.timer_running, "nothing left to wait for");
    }

    #[test]
    fn the_summary_line_lists_only_non_zero_buckets() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "waiting")]));
        s.handle_status(&args(&[("pane_id", "2"), ("status", "working")]));
        s.handle_status(&args(&[("pane_id", "3"), ("status", "working")]));
        assert_eq!(s.summary_line(), "1 waiting \u{b7} 2 working");
    }

    #[test]
    fn an_empty_fleet_summarises_to_nothing() {
        assert_eq!(state().summary_line(), "");
    }

    /// Republishing an unchanged summary would spawn a subprocess every tick.
    #[test]
    fn the_summary_is_published_only_when_it_changes() {
        let mut s = state();
        s.summary_path = "/tmp/summary".into();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "waiting")]));
        s.publish_summary();
        assert_eq!(s.last_summary, "1 waiting");
        s.last_summary = "sentinel".into();
        s.publish_summary();
        assert_eq!(s.last_summary, "1 waiting", "a changed summary republishes");
        s.publish_summary();
        assert_eq!(s.last_summary, "1 waiting");
    }

    /// The documented contract: every key present on every publish, so a
    /// consumer testing one count never has to tell absent from zero.
    #[test]
    fn the_kv_summary_always_carries_every_key() {
        let s = state();
        assert_eq!(s.summary_kv(), "failed=0 waiting=0 working=0 done=0 found=0 total=0");
    }

    #[test]
    fn the_kv_summary_counts_match_the_prose_line() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "waiting")]));
        s.handle_status(&args(&[("pane_id", "2"), ("status", "idlewait")]));
        s.handle_status(&args(&[("pane_id", "3"), ("status", "working")]));
        s.handle_status(&args(&[("pane_id", "4"), ("status", "failed")]));
        assert_eq!(s.summary_line(), "1 failed \u{b7} 2 waiting \u{b7} 1 working");
        assert_eq!(
            s.summary_kv(),
            "failed=1 waiting=2 working=1 done=0 found=0 total=4",
            "the two views must never disagree"
        );
    }

    /// The prose line is empty when nothing is happening, which is what keeps a
    /// status bar clean. The kv line still reports, since a consumer polling it
    /// needs zeros rather than an empty read.
    #[test]
    fn an_idle_fleet_has_an_empty_prose_line_but_a_full_kv_line() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "idle")]));
        assert_eq!(s.summary_line(), "");
        assert_eq!(s.summary_kv(), "failed=0 waiting=0 working=0 done=0 found=0 total=1");
    }

    /// Unconfigured, the publish path must not run at all.
    #[test]
    fn no_summary_path_publishes_nothing() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "waiting")]));
        s.publish_summary();
        assert!(s.last_summary.is_empty());
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

    /// A foreign row is killable, but only through the CLI: the plugin's own
    /// shims act on the current session, so routing one at a foreign pane id
    /// would signal an unrelated pane with the same number.
    #[test]
    fn a_foreign_row_is_killable_through_the_cli_route() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "3"), ("session", "other"), ("status", "working")]));
        s.selected = 0;
        assert!(s.can_kill_selected(), "a live foreign session can be reached");
        assert!(s.selected_is_foreign(), "and must not use the session-local shims");

        s.handle_status(&args(&[("pane_id", "4"), ("session", "mob"), ("status", "working")]));
        s.selected = s.agents.iter().position(|a| a.session() == "mob").unwrap();
        assert!(s.can_kill_selected());
        assert!(!s.selected_is_foreign(), "a home row uses the direct shim");
    }

    /// Nothing is left to signal once the session is gone, in either direction.
    #[test]
    fn kill_is_refused_once_the_session_is_dead() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "3"), ("session", "other"), ("status", "working")]));
        s.selected = 0;
        s.apply_sessions(vec!["mob".into()]);
        assert!(!s.can_kill_selected(), "a dead session has no pane to close");
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

        // A home `waiting` row also needs ticks now, but for its own reason:
        // it must be able to reach the escalation threshold. Once past it there
        // is nothing left to count towards and the clock may stop.
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "3"), ("session", "mob"), ("status", "waiting")]));
        s.timer_running = false;
        s.arm_timer();
        assert!(s.timer_running, "a blocked home row ticks until it escalates");

        s.now = crate::WAITING_ESCALATE_AFTER + 1.0;
        s.timer_running = false;
        s.arm_timer();
        assert!(!s.timer_running, "already escalated: nothing left to wait for");
    }

    /// A decayed foreign row must keep ticking: the clock is what paces the
    /// spool poll, and that poll is the only thing that can bring the row back.
    /// Stopping here is what stranded foreign rows on `unknown`.
    #[test]
    fn a_decayed_foreign_row_keeps_the_clock_running() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "3"), ("session", "other"), ("status", "waiting")]));
        s.timer_running = false;
        s.arm_timer();
        assert!(s.timer_running);

        s.now = STALE_AFTER;
        assert!(s.age_foreign_rows());
        assert_eq!(s.agents[0].status, Status::Unknown);
        s.timer_running = false;
        s.arm_timer();
        assert!(s.timer_running, "the poll still needs ticks to recover it");
    }

    /// The permanent-wakeup guard, rehomed onto the condition that actually
    /// means "nothing can refresh this": the session is gone, so no spool
    /// record will ever arrive and there is nothing left for the clock to do.
    #[test]
    fn a_dead_sessions_row_does_not_hold_the_clock_open() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "3"), ("session", "other"), ("status", "waiting")]));
        s.apply_sessions(vec!["mob".to_string()]);
        assert!(!s.agents[0].session_alive);

        s.now = STALE_AFTER;
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
        // The reason survives the hook, the spool file, the scan and the merge.
        assert_eq!(
            a.block,
            Some(Block::Question),
            "no notification_type is a free-text question"
        );
        // Carried forward by the hook: Notification's payload has neither.
        assert_eq!(a.session_id, "uuid-xyz");
        assert_eq!(a.project(), "web");

        // The blocked agent now writes nothing, exactly as a real one waiting on
        // a prompt does. Re-scanning the unchanged file must keep the row
        // `waiting` well past STALE_AFTER: before the poll and the re-confirm
        // rule this decayed to `unknown` while the agent sat there blocked.
        let rescan = || {
            let out = Command::new("sh")
                .arg("-c")
                .arg(&script)
                .env("ZJ_AGENT_SPOOL_DIR", &spool)
                .output()
                .expect("scan runs");
            crate::discover::parse(&String::from_utf8_lossy(&out.stdout))
        };
        for _ in 0..20 {
            s.now += crate::SPOOL_POLL_INTERVAL;
            assert!(s.spool_poll_due() || s.scan_pending, "the poll stays due");
            s.last_scan_at = s.now;
            let again = rescan();
            assert_eq!(again.spooled.len(), 1, "the record is still on disk");
            s.apply_scan_result(crate::discover::Scan {
                found: found(&[("other", 7)]),
                spooled: again.spooled,
                complete: true,
            });
            s.age_foreign_rows();
        }
        assert!(s.now > STALE_AFTER, "past the decay threshold");
        assert_eq!(
            s.agents[0].status,
            Status::Waiting,
            "a blocked foreign agent stays visible"
        );

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

    /// The headline regression: a foreign agent that keeps working must keep
    /// reading `working`. Nothing but the poll refreshes it, so before the poll
    /// existed this row decayed to `unknown` after a minute and stayed there.
    #[test]
    fn a_foreign_row_refreshed_by_the_poll_does_not_decay() {
        let mut s = state();
        s.apply_scan(found(&[("other", 3)]));
        let mut ts = 100.0_f64;
        for _ in 0..8 {
            s.now += crate::SPOOL_POLL_INTERVAL * 4.0;
            ts += crate::SPOOL_POLL_INTERVAL * 4.0;
            s.apply_scan_result(scan_with(
                found(&[("other", 3)]),
                vec![spool(&[
                    ("ts", &ts.to_string()),
                    ("pane_id", "3"),
                    ("session", "other"),
                    ("status", "working"),
                ])],
            ));
            s.age_foreign_rows();
        }
        assert!(s.now > STALE_AFTER * 2.0, "well past the decay threshold");
        assert_eq!(s.agents[0].status, Status::Working);
    }

    /// With the epoch frozen, ages were measured against a reference that never
    /// moved, so the last record read as current forever and fought the
    /// tick-clock decay in `age_foreign_rows`.
    #[test]
    fn a_frozen_spool_epoch_still_ages_records() {
        let mut s = state();
        s.apply_scan(found(&[("other", 3)]));
        let rec = || {
            spool(&[
                ("ts", "100"),
                ("pane_id", "3"),
                ("session", "other"),
                ("status", "working"),
            ])
        };
        assert!(s.apply_scan_result(scan_with(found(&[("other", 3)]), vec![rec()])));
        assert_eq!(s.agents[0].status, Status::Working);

        s.agents[0].status = Status::Unknown;
        s.now += STALE_AFTER;
        assert!(
            !s.apply_scan_result(scan_with(found(&[("other", 3)]), vec![rec()])),
            "a record that has not moved in STALE_AFTER is stale, epoch frozen or not"
        );
        assert_eq!(s.agents[0].status, Status::Unknown);
    }

    /// A blocked agent writes nothing while it waits, so its record never
    /// advances. Re-reading it still confirms the state: silence is what
    /// `waiting` predicts.
    #[test]
    fn a_quiet_waiting_foreign_row_survives_the_poll() {
        let mut s = state();
        s.apply_scan(found(&[("other", 3)]));
        let rec = || {
            spool(&[
                ("ts", "100"),
                ("pane_id", "3"),
                ("session", "other"),
                ("status", "waiting"),
            ])
        };
        s.apply_scan_result(scan_with(found(&[("other", 3)]), vec![rec()]));
        assert_eq!(s.agents[0].status, Status::Waiting);

        for _ in 0..20 {
            s.now += crate::SPOOL_POLL_INTERVAL;
            s.apply_scan_result(scan_with(found(&[("other", 3)]), vec![rec()]));
            s.age_foreign_rows();
        }
        assert!(s.now > STALE_AFTER, "past the decay threshold");
        assert_eq!(s.agents[0].status, Status::Waiting, "still blocked on you");
    }

    /// The other half of the rule: `working` claims active progress, and an
    /// unchanging record is evidence against it rather than for it.
    #[test]
    fn a_quiet_working_foreign_row_still_decays() {
        let mut s = state();
        s.apply_scan(found(&[("other", 3)]));
        let rec = || {
            spool(&[
                ("ts", "100"),
                ("pane_id", "3"),
                ("session", "other"),
                ("status", "working"),
            ])
        };
        s.apply_scan_result(scan_with(found(&[("other", 3)]), vec![rec()]));
        assert_eq!(s.agents[0].status, Status::Working);

        for _ in 0..20 {
            s.now += crate::SPOOL_POLL_INTERVAL;
            s.apply_scan_result(scan_with(found(&[("other", 3)]), vec![rec()]));
            s.age_foreign_rows();
        }
        assert_eq!(s.agents[0].status, Status::Unknown);
    }

    #[test]
    fn the_poll_fires_on_a_cadence_only_while_a_foreign_row_exists() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "4"), ("session", "mob"), ("status", "working")]));
        s.now = crate::SPOOL_POLL_INTERVAL * 10.0;
        assert!(!s.spool_poll_due(), "a home-only panel never polls");

        s.apply_scan(found(&[("other", 3)]));
        s.last_scan_at = s.now;
        assert!(!s.spool_poll_due(), "not due yet");

        s.now += crate::SPOOL_POLL_INTERVAL;
        assert!(s.spool_poll_due());

        s.scan_pending = true;
        assert!(!s.spool_poll_due(), "a scan is already in flight");
    }

    /// Nothing will ever refresh a row whose session is gone, so polling for it
    /// is wasted work.
    #[test]
    fn the_poll_ignores_rows_whose_session_is_gone() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "3"), ("session", "other"), ("status", "working")]));
        s.now = crate::SPOOL_POLL_INTERVAL * 10.0;
        assert!(s.spool_poll_due());

        s.apply_sessions(vec!["mob".to_string()]);
        assert!(!s.spool_poll_due(), "the session is gone");
    }

    /// The re-confirm rule must not undo `apply_sessions`. A dead session's
    /// processes are gone, so the scan drops the row and no record can reach
    /// it - but a leftover file plus a surviving row must not read `waiting`.
    #[test]
    fn a_reconfirm_cannot_revive_a_dead_sessions_row() {
        let mut s = state();
        s.apply_scan(found(&[("other", 3)]));
        let rec = || {
            spool(&[
                ("ts", "100"),
                ("pane_id", "3"),
                ("session", "other"),
                ("status", "waiting"),
            ])
        };
        s.apply_scan_result(scan_with(found(&[("other", 3)]), vec![rec()]));
        assert_eq!(s.agents[0].status, Status::Waiting);

        s.apply_sessions(vec!["mob".to_string()]);
        assert_eq!(s.agents[0].status, Status::Unknown);

        s.now += STALE_AFTER;
        s.apply_scan_result(scan_with(Vec::new(), vec![rec()]));
        assert!(
            s.agents.is_empty() || s.agents[0].status == Status::Unknown,
            "a dead session's row never comes back as waiting"
        );
    }

    /// The clock must actually drive the poll. Everything else here tests
    /// `apply_scan_result` directly, which would pass just as happily with the
    /// poll never wired into the tick at all.
    #[test]
    fn the_tick_dispatches_the_poll() {
        let mut s = state();
        s.apply_scan(found(&[("other", 3)]));
        s.scan_pending = false;
        s.last_scan_at = s.now;

        let ticks = (crate::SPOOL_POLL_INTERVAL / TICK) as usize;
        for _ in 0..ticks - 1 {
            s.on_tick();
            assert!(!s.scan_pending, "not due yet");
        }
        s.on_tick();
        assert!(s.scan_pending, "the tick must dispatch a scan once the poll is due");
        assert_eq!(s.last_scan_at, s.now);
    }

    /// A home-only panel must not acquire a background scan loop.
    #[test]
    fn the_tick_does_not_poll_without_a_foreign_row() {
        let mut s = state();
        s.handle_status(&args(&[("pane_id", "4"), ("session", "mob"), ("status", "working")]));
        for _ in 0..(crate::SPOOL_POLL_INTERVAL / TICK) as usize * 3 {
            s.on_tick();
        }
        assert!(!s.scan_pending, "nothing foreign to poll for");
    }

    /// `request_scan` stamps on dispatch, so a scan that never comes back
    /// cannot wedge the poll behind a `last_scan_at` that never moves.
    #[test]
    fn the_poll_clock_is_stamped_on_dispatch() {
        let mut s = state();
        s.apply_scan(found(&[("other", 3)]));
        s.now = 42.0;
        s.request_scan();
        assert_eq!(s.last_scan_at, 42.0);
        assert!(s.scan_pending);
    }

    /// The marker exists to survive the trip back from a banner, not past that.
    #[test]
    fn focusing_the_panel_clears_the_notified_markers() {
        let mut s = State::default();
        s.notifier.binary = "osascript".into();
        s.handle_status(&args(&[("pane_id", "1"), ("status", "working")]));
        s.handle_status(&args(&[("pane_id", "1"), ("status", "waiting")]));
        assert!(s.agents[0].notified, "a notified transition marks its row");
        assert!(s.clear_notified(), "clearing reports the change");
        assert!(!s.agents[0].notified);
        assert!(!s.clear_notified(), "a second clear is a no-op");
    }

    /// A suppressed notification must not mark the row: nothing fired, so there
    /// is nothing to come back to.
    #[test]
    fn a_suppressed_notification_leaves_the_row_unmarked() {
        let mut s = State::default();
        s.notifier.binary = "osascript".into();
        s.notifier.focused = true;
        s.handle_status(&args(&[("pane_id", "1"), ("status", "working")]));
        s.handle_status(&args(&[("pane_id", "1"), ("status", "waiting")]));
        assert!(!s.agents[0].notified);
    }
}
