//! Desktop notifications when an agent needs you.
//!
//! The panel already knows the moment an agent blocks, but `show_self` only
//! raises a floating pane inside Zellij - useless when your attention is on a
//! browser or another window, which is exactly when it matters. This fires a
//! real system notification through whatever notifier the host has.

use crate::agent::AgentId;
use crate::host;
use crate::status::Status;

pub(crate) const CTX_DETECT: &str = "notify-detect";

/// Probes for each notifier in preference order and prints the first hit.
/// `terminal-notifier` wins on macOS when present: it groups notifications, so
/// a burst replaces rather than stacks.
pub(crate) fn detect_script() -> &'static str {
    "for n in terminal-notifier osascript notify-send; do \
     command -v \"$n\" >/dev/null 2>&1 && { printf '%s' \"$n\"; exit 0; }; done; \
     printf 'none'"
}

/// Which transitions are worth interrupting the user for.
///
/// `done` is absent by default on purpose: it is the high-frequency trigger and
/// the one most likely to make the whole feature annoying enough to switch off.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Triggers(Vec<Status>);

impl Default for Triggers {
    fn default() -> Self {
        Triggers(vec![Status::Waiting, Status::Failed])
    }
}

impl Triggers {
    /// An explicit empty string disables notifications; an unset key keeps the
    /// default. Unknown names are ignored rather than failing the whole config:
    /// a typo should cost one trigger, not every notification.
    pub(crate) fn parse(spec: &str) -> Self {
        Triggers(
            spec.split(',')
                .filter_map(|s| match s.trim() {
                    "waiting" => Some(Status::Waiting),
                    "idlewait" | "idle-wait" => Some(Status::IdleWait),
                    "failed" => Some(Status::Failed),
                    "done" => Some(Status::Done),
                    _ => None,
                })
                .collect(),
        )
    }

    pub(crate) fn wants(&self, status: Status) -> bool {
        self.0.contains(&status)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// A transition waiting to be sent, held briefly so a burst becomes one banner.
pub(crate) struct Pending {
    pub(crate) id: AgentId,
    pub(crate) status: Status,
    pub(crate) task: String,
}

#[derive(Default)]
pub(crate) struct Notifier {
    /// The detected binary, or empty until the probe returns. `none` means the
    /// probe ran and found nothing, which is different from not having run.
    pub(crate) binary: String,
    pub(crate) triggers: Triggers,
    pub(crate) cooldown: f64,
    pub(crate) sound: bool,
    /// Suppresses notifications while the panel is visible and focused: the
    /// panel is already doing the job a banner would do.
    pub(crate) focused: bool,
    pub(crate) pending: Vec<Pending>,
    /// When the current coalescing window closes, on the panel's tick clock.
    pub(crate) flush_at: Option<f64>,
    /// Last notification per agent, so a flapping row cannot fire repeatedly.
    sent: Vec<(AgentId, f64)>,
}

/// How long a burst is held before it is sent as one summary. Long enough to
/// catch a fan-out landing together, short enough to stay a live signal.
const COALESCE: f64 = 2.0;

impl Notifier {
    pub(crate) fn enabled(&self) -> bool {
        !self.triggers.is_empty() && self.binary != "none" && !self.binary.is_empty()
    }

    /// Records a transition, subject to the per-agent cooldown. Returns whether
    /// anything was queued, so the caller knows to arm the flush timer.
    pub(crate) fn queue(&mut self, id: &AgentId, status: Status, task: &str, now: f64) -> bool {
        if !self.enabled() || !self.triggers.wants(status) || self.focused {
            return false;
        }
        if let Some((_, at)) = self.sent.iter().find(|(a, _)| a == id) {
            if now - *at < self.cooldown {
                return false;
            }
        }
        // A second transition for the same agent inside one window replaces the
        // first: only the newest state is worth telling anyone about.
        self.pending.retain(|p| &p.id != id);
        self.pending.push(Pending {
            id: id.clone(),
            status,
            task: task.to_string(),
        });
        if self.flush_at.is_none() {
            self.flush_at = Some(now + COALESCE);
        }
        true
    }

    /// Sends the queued batch if its window has closed. One banner for one
    /// agent, a count for several.
    pub(crate) fn flush(&mut self, now: f64) {
        match self.flush_at {
            Some(at) if now >= at => {}
            _ => return,
        }
        self.flush_at = None;
        let batch = std::mem::take(&mut self.pending);
        if batch.is_empty() {
            return;
        }
        for p in &batch {
            self.sent.retain(|(a, _)| a != &p.id);
            self.sent.push((p.id.clone(), now));
        }
        let (title, body) = message(&batch);
        host::notify(&self.binary, &title, &body, self.sound);
    }

    /// Drops cooldown entries for agents that no longer exist, so the list
    /// cannot grow without bound across a long-lived panel.
    pub(crate) fn retain_known(&mut self, known: &[AgentId]) {
        self.sent.retain(|(a, _)| known.contains(a));
        self.pending.retain(|p| known.contains(&p.id));
    }
}

/// The banner text. A single agent names its task, since that is what tells you
/// whether to go now; a burst gets a count, because five stacked banners is the
/// failure mode the measurements found.
fn message(batch: &[Pending]) -> (String, String) {
    if batch.len() == 1 {
        let p = &batch[0];
        let what = match p.status {
            Status::Failed => "failed",
            Status::Done => "finished",
            _ => "needs input",
        };
        let where_ = format!("{} \u{b7} pane {}", p.id.session, p.id.pane_id);
        let body = match p.task.is_empty() {
            true => where_,
            false => format!("{} \u{2014} {}", p.task, where_),
        };
        return (format!("Agent {}", what), body);
    }
    let blocked = batch
        .iter()
        .filter(|p| matches!(p.status, Status::Waiting | Status::IdleWait))
        .count();
    let failed = batch.iter().filter(|p| p.status == Status::Failed).count();
    let mut parts = Vec::new();
    if blocked > 0 {
        parts.push(format!("{} need input", blocked));
    }
    if failed > 0 {
        parts.push(format!("{} failed", failed));
    }
    let done = batch.len() - blocked - failed;
    if done > 0 {
        parts.push(format!("{} finished", done));
    }
    (format!("{} agents", batch.len()), parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(session: &str, pane_id: u32) -> AgentId {
        AgentId {
            session: session.into(),
            pane_id,
        }
    }

    fn notifier() -> Notifier {
        Notifier {
            binary: "osascript".into(),
            cooldown: 60.0,
            ..Default::default()
        }
    }

    #[test]
    fn defaults_to_waiting_and_failed_but_not_done() {
        let t = Triggers::default();
        assert!(t.wants(Status::Waiting));
        assert!(t.wants(Status::Failed));
        assert!(!t.wants(Status::Done), "done is opt-in: it is the noisy one");
    }

    #[test]
    fn an_empty_spec_disables_notifications() {
        let mut n = notifier();
        n.triggers = Triggers::parse("");
        assert!(!n.enabled());
    }

    #[test]
    fn unknown_trigger_names_are_ignored_not_fatal() {
        let t = Triggers::parse("waiting,nonsense,done");
        assert!(t.wants(Status::Waiting) && t.wants(Status::Done));
        assert!(!t.wants(Status::Failed), "not listed");
    }

    /// Nothing detected must degrade to silence rather than firing a command
    /// that does not exist.
    #[test]
    fn no_notifier_means_disabled() {
        let mut n = notifier();
        n.binary = "none".into();
        assert!(!n.enabled());
        n.binary = String::new();
        assert!(!n.enabled(), "an unfinished probe is not a notifier either");
    }

    #[test]
    fn a_queued_transition_arms_the_flush_window() {
        let mut n = notifier();
        assert!(n.queue(&id("mob", 3), Status::Waiting, "Fix it", 0.0));
        assert_eq!(n.flush_at, Some(COALESCE));
        n.flush(0.5);
        assert_eq!(n.pending.len(), 1, "the window has not closed yet");
        n.flush(COALESCE);
        assert!(n.pending.is_empty() && n.flush_at.is_none());
    }

    /// The measured failure: five agents blocking together stacking five
    /// separate banners.
    #[test]
    fn a_burst_coalesces_into_one_batch() {
        let mut n = notifier();
        for pane in 1..=5 {
            n.queue(&id("mob", pane), Status::Waiting, "t", 0.1);
        }
        assert_eq!(n.pending.len(), 5);
        let (title, body) = message(&n.pending);
        assert_eq!(title, "5 agents");
        assert_eq!(body, "5 need input");
    }

    #[test]
    fn a_single_agent_names_its_task_and_location() {
        let batch = vec![Pending {
            id: id("api", 7),
            status: Status::Waiting,
            task: "Fix flaky checkout test".into(),
        }];
        let (title, body) = message(&batch);
        assert_eq!(title, "Agent needs input");
        assert!(body.contains("Fix flaky checkout test"), "{:?}", body);
        assert!(body.contains("api"), "the session locates it: {:?}", body);
        assert!(body.contains("pane 7"), "{:?}", body);
    }

    /// With no task there is nothing to say about the work, but the location is
    /// still what gets you there.
    #[test]
    fn a_taskless_agent_still_reports_where_it_is() {
        let batch = vec![Pending {
            id: id("api", 7),
            status: Status::Failed,
            task: String::new(),
        }];
        let (title, body) = message(&batch);
        assert_eq!(title, "Agent failed");
        assert_eq!(body, "api \u{b7} pane 7");
    }

    #[test]
    fn a_mixed_burst_breaks_down_by_kind() {
        let batch = vec![
            Pending {
                id: id("mob", 1),
                status: Status::Waiting,
                task: String::new(),
            },
            Pending {
                id: id("mob", 2),
                status: Status::Failed,
                task: String::new(),
            },
            Pending {
                id: id("mob", 3),
                status: Status::IdleWait,
                task: String::new(),
            },
        ];
        let (title, body) = message(&batch);
        assert_eq!(title, "3 agents");
        assert_eq!(body, "2 need input, 1 failed");
    }

    /// An agent flipping waiting -> working -> waiting while you are mid-approval
    /// must not fire twice.
    #[test]
    fn the_cooldown_suppresses_a_flapping_agent() {
        let mut n = notifier();
        n.queue(&id("mob", 3), Status::Waiting, "t", 0.0);
        n.flush(COALESCE);
        assert!(
            !n.queue(&id("mob", 3), Status::Waiting, "t", 10.0),
            "still inside the cooldown"
        );
        assert!(
            n.queue(&id("mob", 3), Status::Waiting, "t", 70.0),
            "past the cooldown it may fire again"
        );
    }

    /// The cooldown is per agent, so one noisy row must not silence the others.
    #[test]
    fn the_cooldown_is_scoped_to_one_agent() {
        let mut n = notifier();
        n.queue(&id("mob", 3), Status::Waiting, "t", 0.0);
        n.flush(COALESCE);
        assert!(n.queue(&id("mob", 4), Status::Waiting, "t", 10.0));
    }

    /// Same pane number in two sessions is a different agent.
    #[test]
    fn the_cooldown_distinguishes_sessions() {
        let mut n = notifier();
        n.queue(&id("mob", 3), Status::Waiting, "t", 0.0);
        n.flush(COALESCE);
        assert!(n.queue(&id("other", 3), Status::Waiting, "t", 10.0));
    }

    /// The panel being up and focused already tells you; a banner would be noise.
    #[test]
    fn a_focused_panel_suppresses_notifications() {
        let mut n = notifier();
        n.focused = true;
        assert!(!n.queue(&id("mob", 3), Status::Waiting, "t", 0.0));
    }

    /// Only the newest state for an agent is worth sending.
    #[test]
    fn a_second_transition_replaces_the_first_in_the_window() {
        let mut n = notifier();
        n.triggers = Triggers::parse("waiting,failed");
        n.queue(&id("mob", 3), Status::Waiting, "t", 0.0);
        n.queue(&id("mob", 3), Status::Failed, "t", 0.5);
        assert_eq!(n.pending.len(), 1);
        assert_eq!(n.pending[0].status, Status::Failed);
    }

    /// A statuses the user did not ask about must never fire.
    #[test]
    fn untriggered_statuses_are_ignored() {
        let mut n = notifier();
        assert!(!n.queue(&id("mob", 3), Status::Working, "t", 0.0));
        assert!(!n.queue(&id("mob", 3), Status::Done, "t", 0.0));
        assert!(n.pending.is_empty());
    }

    /// A long-lived panel must not accumulate cooldown entries for agents that
    /// exited long ago.
    #[test]
    fn cooldown_entries_are_dropped_with_their_agents() {
        let mut n = notifier();
        n.queue(&id("mob", 3), Status::Waiting, "t", 0.0);
        n.flush(COALESCE);
        n.retain_known(&[]);
        assert!(
            n.queue(&id("mob", 3), Status::Waiting, "t", 1.0),
            "a forgotten agent starts fresh"
        );
    }

    #[test]
    fn flushing_an_empty_queue_sends_nothing() {
        let mut n = notifier();
        n.flush(100.0);
        assert!(n.pending.is_empty() && n.flush_at.is_none());
    }

    /// The probe must name every notifier the host shim knows how to drive.
    #[test]
    fn the_probe_covers_every_supported_notifier() {
        let s = detect_script();
        for n in ["terminal-notifier", "osascript", "notify-send"] {
            assert!(s.contains(n), "probe missing {}: {}", n, s);
        }
    }
}
