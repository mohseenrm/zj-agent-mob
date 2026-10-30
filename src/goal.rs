use serde::{Deserialize, Serialize};
use zellij_tile::prelude::Text;

use crate::style::DIM_LEVEL;
pub(crate) use crate::util::truncate_cells as fit;

pub(crate) const SOURCE: &str = "codex-app-server-v1";
pub(crate) const MAX_PAYLOAD: usize = 32768;
pub(crate) const ADAPTER: &str = include_str!("../scripts/zj-agent-mob-goals.py");

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Layout {
    #[default]
    Off,
    Inline,
    Inspector,
}

impl Layout {
    pub(crate) fn parse(value: Option<&String>) -> Self {
        match value.map(String::as_str) {
            Some("inline") => Self::Inline,
            Some("inspector") => Self::Inspector,
            _ => Self::Off,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Status {
    Active,
    Paused,
    Blocked,
    UsageLimited,
    BudgetLimited,
    Complete,
}

impl Status {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Blocked => "blocked",
            Self::UsageLimited => "usage-limited",
            Self::BudgetLimited => "budget-limited",
            Self::Complete => "complete",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Goal {
    pub(crate) thread_id: String,
    pub(crate) objective: String,
    pub(crate) status: Status,
    pub(crate) tokens_used: u64,
    pub(crate) token_budget: Option<u64>,
    pub(crate) time_used_seconds: u64,
    pub(crate) created_at: u64,
    pub(crate) updated_at: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub(crate) schema_version: u8,
    pub(crate) source: String,
    pub(crate) session: String,
    pub(crate) pane_id: u32,
    pub(crate) server_pid: u32,
    #[serde(default)]
    pub(crate) agent_pid: u32,
    pub(crate) server_started: String,
    pub(crate) agent_session_id: String,
    pub(crate) observed_at: u64,
    #[serde(default)]
    pub(crate) age_seconds: u64,
    #[serde(default = "instance_valid")]
    pub(crate) instance_valid: bool,
    pub(crate) available: bool,
    pub(crate) goal: Option<Goal>,
}

fn instance_valid() -> bool {
    true
}

impl Snapshot {
    pub(crate) fn parse(payload: &str) -> Option<Self> {
        if payload.len() > MAX_PAYLOAD {
            return None;
        }
        let mut value: Self = serde_json::from_str(payload).ok()?;
        if value.schema_version != 1
            || value.source != SOURCE
            || value.observed_at == 0
            || value.server_pid == 0
            || value.server_started.is_empty()
            || value.agent_session_id.len() > 256
            || value.session.len() > 256
            || value.server_started.len() > 128
        {
            return None;
        }
        if let Some(goal) = &mut value.goal {
            if !value.available
                || !value.instance_valid
                || value.agent_pid == 0
                || goal.thread_id != value.agent_session_id
                || goal.thread_id.is_empty()
                || goal.objective.is_empty()
                || goal.objective.chars().count() > 4096
                || goal.updated_at < goal.created_at
            {
                return None;
            }
            goal.objective = clean(&goal.objective);
        }
        Some(value)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Observation {
    pub(crate) snapshot: Snapshot,
    pub(crate) seen_at: f64,
    pub(crate) unavailable: bool,
    pub(crate) last_observed: u64,
}

impl crate::state::State {
    pub(crate) fn goal_refresh_needed(&self) -> bool {
        self.goal_layout != Layout::Off
            && self.discover
            && self
                .agents
                .iter()
                .any(|a| a.tool == "codex" && a.alive && a.session_alive)
    }

    pub(crate) fn goal_owners(&self) -> String {
        if !self.goal_refresh_needed() {
            return String::new();
        }
        let owners: Vec<_> = self
            .agents
            .iter()
            .filter(|a| {
                a.tool == "codex"
                    && a.alive
                    && a.session_alive
                    && a.server_pid != 0
                    && a.agent_pid != 0
                    && !a.server_started.is_empty()
                    && !a.session_id.is_empty()
            })
            .take(128)
            .map(|a| {
                serde_json::json!({"session": a.session(), "pane_id": a.pane_id(),
                "server_pid": a.server_pid, "server_started": a.server_started,
                "agent_pid": a.agent_pid, "report_ts": a.spool_ts, "agent_session_id": a.session_id})
            })
            .collect();
        if owners.is_empty() {
            String::new()
        } else {
            serde_json::to_string(&owners).unwrap_or_default()
        }
    }

    pub(crate) fn handle_goal(&mut self, payload: &str) -> bool {
        Snapshot::parse(payload).is_some_and(|s| self.apply_goal(s))
    }

    pub(crate) fn apply_goal(&mut self, snapshot: Snapshot) -> bool {
        if self.goal_layout == Layout::Off {
            return false;
        }
        let Some(agent) = self.agents.iter_mut().find(|a| {
            a.tool == "codex"
                && a.alive
                && a.session_alive
                && a.pane_id() == snapshot.pane_id
                && a.server_pid == snapshot.server_pid
                && a.agent_pid == snapshot.agent_pid
                && a.server_started == snapshot.server_started
                && a.session_id == snapshot.agent_session_id
        }) else {
            return false;
        };
        if let Some(previous) = &agent.goal {
            if snapshot.observed_at <= previous.last_observed {
                return false;
            }
            if let (Some(old), Some(new)) = (&previous.snapshot.goal, &snapshot.goal) {
                if new.created_at < old.created_at
                    || (new.created_at == old.created_at && new.updated_at < old.updated_at)
                {
                    return false;
                }
            }
        }
        if !snapshot.instance_valid {
            agent.goal = None;
            agent.agent_pid = 0;
            agent.session_id.clear();
            agent.server_started.clear();
            return true;
        }
        if !snapshot.available {
            if let Some(previous) = &mut agent.goal {
                previous.unavailable = true;
                previous.last_observed = snapshot.observed_at;
                return true;
            }
        }
        agent.goal = Some(Observation {
            seen_at: self.now - snapshot.age_seconds as f64,
            unavailable: !snapshot.available,
            last_observed: snapshot.observed_at,
            snapshot,
        });
        self.arm_timer();
        true
    }

    pub(crate) fn goal_row(&self, agent: &crate::agent::Agent, width: usize, objective: bool) -> Option<Text> {
        if self.goal_layout == Layout::Off {
            return None;
        }
        match &agent.goal {
            Some(o) if o.snapshot.available && o.snapshot.goal.is_none() && !o.unavailable => None,
            Some(o) => Some(o.line(self.now, width, objective)),
            None => Some(Text::new(fit("    goal unavailable", width)).color_range(DIM_LEVEL, ..)),
        }
    }
}

impl Observation {
    pub(crate) fn label(&self, now: f64) -> String {
        if self.snapshot.goal.is_none() {
            return "goal unavailable".to_string();
        }
        let status = self
            .snapshot
            .goal
            .as_ref()
            .map(|g| g.status.label())
            .unwrap_or("unavailable");
        let suffix = if self.unavailable {
            " · unavailable"
        } else if now - self.seen_at >= crate::STALE_AFTER {
            " · old data"
        } else {
            ""
        };
        format!("goal: {}{}", status, suffix)
    }

    pub(crate) fn line(&self, now: f64, width: usize, objective: bool) -> Text {
        let mut label = format!("    {}", self.label(now));
        if let Some(goal) = &self.snapshot.goal {
            if objective {
                let tail = if width >= 100 {
                    format!(" · {}", usage(goal))
                } else {
                    String::new()
                };
                let room = width.saturating_sub(cells(&label) + cells(&tail) + 3);
                if room > 2 {
                    label.push_str(&format!(" · {}{}", fit(&goal.objective, room), tail));
                }
            }
        }
        let color = match self.snapshot.goal.as_ref().map(|g| &g.status) {
            Some(Status::Complete) => 3,
            Some(Status::Blocked | Status::BudgetLimited | Status::UsageLimited) => 1,
            Some(Status::Active) => 0,
            Some(Status::Paused) => 2,
            _ => DIM_LEVEL,
        };
        let end = 4 + self.label(now).chars().count();
        Text::new(fit(&label, width)).color_range(color, 4..end)
    }

    pub(crate) fn inspector(&self, now: f64, width: usize) -> Vec<Text> {
        let mut lines = vec![self.line(now, width, false)];
        if let Some(goal) = &self.snapshot.goal {
            let available = width.saturating_sub(4).max(1);
            let mut remaining = goal.objective.as_str();
            // Bound the inspector so the selected row and keyboard hints survive.
            for index in 0..2 {
                if remaining.is_empty() {
                    break;
                }
                let line = fit(remaining, available);
                lines.push(Text::new(fit(&format!("    {}", line), width)));
                if cells(remaining) <= available {
                    break;
                }
                if index == 0 {
                    let taken = remaining
                        .chars()
                        .scan(0, |n, c| {
                            *n += unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
                            Some(*n)
                        })
                        .take_while(|n| *n <= available.saturating_sub(1))
                        .count();
                    remaining = &remaining[remaining
                        .char_indices()
                        .nth(taken)
                        .map(|(i, _)| i)
                        .unwrap_or(remaining.len())..];
                }
            }
            lines.push(Text::new(fit(
                &format!(
                    "    {} · {} elapsed",
                    usage(goal),
                    crate::util::fmt_elapsed(goal.time_used_seconds as f64)
                ),
                width,
            )));
        }
        lines
    }
}

fn usage(goal: &Goal) -> String {
    match goal.token_budget {
        Some(budget) => format!(
            "{} / {} tokens{}",
            goal.tokens_used,
            budget,
            if goal.tokens_used >= budget {
                " · budget reached"
            } else {
                ""
            }
        ),
        None => format!("{} tokens · no token budget", goal.tokens_used),
    }
}

fn cells(value: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(value)
}

fn clean(value: &str) -> String {
    let mut out = String::new();
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.next() {
                Some('[') => {
                    for x in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&x) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(x) = chars.next() {
                        if x == '\u{7}' || (x == '\u{1b}' && chars.peek() == Some(&'\\')) {
                            if x == '\u{1b}' {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else if c.is_whitespace() {
            out.push(' ');
        } else if !c.is_control() && !matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Find, State};
    use crate::util::testing::item_text;
    use serde_json::{json, Value};

    fn fixture(name: &str, order: u64) -> String {
        let fixtures: Value = serde_json::from_str(include_str!("../tests/fixtures/goals.json")).unwrap();
        json!({"schema_version":1,"source":SOURCE,"session":"mob","pane_id":3,
            "server_pid":42,"agent_pid":77,"server_started":"Fri Oct 30 12:00:00 2026","agent_session_id":"qa-thread",
            "observed_at":order,"available":true,"goal":fixtures[name]["result"]["goal"]})
        .to_string()
    }

    fn state() -> State {
        let mut state = State {
            goal_layout: Layout::Inline,
            discover: true,
            session_name: "mob".into(),
            ..Default::default()
        };
        let pairs = [
            ("pane_id", "3"),
            ("session", "mob"),
            ("tool", "codex"),
            ("status", "done"),
            ("session_id", "qa-thread"),
            ("server_pid", "42"),
            ("agent_pid", "77"),
            ("ts", "1793388340"),
            ("server_started", "Fri Oct 30 12:00:00 2026"),
        ];
        state.handle_status(&pairs.into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect());
        state
    }

    #[test]
    fn real_source_fixtures_parse_every_state_and_explicit_no_goal() {
        for name in [
            "active",
            "paused",
            "blocked",
            "complete",
            "replacement",
            "budget",
            "usage",
            "none",
            "cleared",
        ] {
            let s = Snapshot::parse(&fixture(name, 1)).unwrap();
            assert_eq!(s.goal.is_none(), matches!(name, "none" | "cleared"));
        }
    }

    #[test]
    fn activity_and_goal_lifecycles_are_independent() {
        let mut s = state();
        assert!(s.handle_goal(&fixture("active", 1)));
        assert_eq!(s.agents[0].status, crate::status::Status::Done);
        s.handle_status(
            &[
                ("pane_id".into(), "3".into()),
                ("status".into(), "waiting".into()),
                ("block".into(), "question".into()),
            ]
            .into(),
        );
        assert_eq!(
            s.agents[0]
                .goal
                .as_ref()
                .unwrap()
                .snapshot
                .goal
                .as_ref()
                .unwrap()
                .status,
            Status::Active
        );
        assert!(s.handle_goal(&fixture("complete", 2)));
        assert_eq!(s.agents[0].status, crate::status::Status::Waiting);
        assert_eq!(
            s.agents[0]
                .goal
                .as_ref()
                .unwrap()
                .snapshot
                .goal
                .as_ref()
                .unwrap()
                .status,
            Status::Complete
        );
    }

    #[test]
    fn dismiss_and_pane_visits_change_only_the_activity_badge() {
        for key in ['d', '\n'] {
            let mut s = state();
            s.permissions_granted = true;
            s.handle_goal(&fixture("complete", 1));
            s.handle_key(crate::testing::key(key));
            assert_eq!(s.agents[0].status, crate::status::Status::Idle);
            assert_eq!(
                s.agents[0]
                    .goal
                    .as_ref()
                    .unwrap()
                    .snapshot
                    .goal
                    .as_ref()
                    .unwrap()
                    .status,
                Status::Complete
            );
        }
    }

    #[test]
    fn repeated_same_second_reads_apply_in_serialized_observation_order() {
        let mut s = state();
        assert!(s.handle_goal(&fixture("active", 1)));
        assert!(s.handle_goal(&fixture("paused", 2)));
        assert!(!s.handle_goal(&fixture("complete", 1)));
        assert!(!s.handle_goal(&fixture("active", 2)));
        assert!(s.handle_goal(&fixture("replacement", 3)));
        assert!(s.agents[0]
            .goal
            .as_ref()
            .unwrap()
            .snapshot
            .goal
            .as_ref()
            .unwrap()
            .objective
            .starts_with("Replacement"));
        assert!(s.handle_goal(&fixture("cleared", 4)));
        assert!(!s.handle_goal(&fixture("complete", 3)));
        assert!(s.goal_row(&s.agents[0], 100, true).is_none());
    }

    #[test]
    fn old_source_revisions_and_generations_cannot_replace_new_truth() {
        let mut s = state();
        s.handle_goal(&fixture("active", 1));
        for field in ["updatedAt", "createdAt"] {
            let mut old: Value = serde_json::from_str(&fixture("complete", 2)).unwrap();
            old["goal"][field] = json!(1793388338u64);
            assert!(!s.handle_goal(&old.to_string()));
        }
    }

    #[test]
    fn malformed_failed_oversized_and_foreign_owners_are_rejected() {
        let mut s = state();
        for (key, value) in [
            ("source", json!("unknown")),
            ("schema_version", json!(2)),
            ("server_pid", json!(99)),
            ("server_started", json!("reused PID")),
            ("pane_id", json!(99)),
            ("agent_session_id", json!("other-thread")),
        ] {
            let mut raw: Value = serde_json::from_str(&fixture("complete", 1)).unwrap();
            raw[key] = value;
            assert!(!s.handle_goal(&raw.to_string()), "{key}");
        }
        assert!(!s.handle_goal("{not json}"));
        assert!(!s.handle_goal(&"x".repeat(MAX_PAYLOAD + 1)));
        let fixtures: Value = serde_json::from_str(include_str!("../tests/fixtures/goals.json")).unwrap();
        assert!(!s.handle_goal(&fixtures["failure"].to_string()));
        for value in [json!(-1), json!("12"), json!(null)] {
            let mut raw: Value = serde_json::from_str(&fixture("active", 1)).unwrap();
            raw["goal"]["tokensUsed"] = value;
            assert!(!s.handle_goal(&raw.to_string()));
        }
    }

    #[test]
    fn unavailable_preserves_truth_and_age_while_no_goal_clears() {
        let mut s = state();
        s.handle_goal(&fixture("active", 1));
        s.now = crate::STALE_AFTER + 1.0;
        assert!(s.agents[0].goal.as_ref().unwrap().label(s.now).contains("old data"));
        let mut raw: Value = serde_json::from_str(&fixture("none", 2)).unwrap();
        raw["available"] = json!(false);
        assert!(s.handle_goal(&raw.to_string()));
        let o = s.agents[0].goal.as_ref().unwrap();
        assert_eq!(o.snapshot.goal.as_ref().unwrap().status, Status::Active);
        assert_eq!(o.seen_at, 0.0);
        assert!(o.label(s.now).contains("unavailable"));
        assert!(!s.handle_goal(&fixture("complete", 1)));
        s.handle_goal(&fixture("none", 3));
        assert!(s.goal_row(&s.agents[0], 80, true).is_none());
    }

    #[test]
    fn new_agent_and_server_instance_retire_old_goal_ownership() {
        for (key, value) in [
            ("session_id", "new-thread"),
            ("server_started", "new server start"),
            ("agent_pid", "88"),
        ] {
            let mut s = state();
            s.handle_goal(&fixture("active", 1));
            s.handle_status(
                &[
                    ("pane_id".into(), "3".into()),
                    ("status".into(), "working".into()),
                    (key.into(), value.into()),
                ]
                .into(),
            );
            assert!(s.agents[0].goal.is_none());
            assert!(!s.handle_goal(&fixture("complete", 2)));
        }
    }

    #[test]
    fn a_confirmed_dead_server_instance_cannot_keep_its_cached_goal() {
        let mut s = state();
        s.handle_goal(&fixture("active", 2));
        let mut raw: Value = serde_json::from_str(&fixture("none", 1)).unwrap();
        raw["available"] = json!(false);
        raw["instance_valid"] = json!(false);
        assert!(!s.handle_goal(&raw.to_string()));
        raw["observed_at"] = json!(3);
        assert!(s.handle_goal(&raw.to_string()));
        assert!(s.agents[0].goal.is_none());
        assert!(s.agents[0].session_id.is_empty());
        assert!(!s.handle_goal(&fixture("complete", 4)));
    }

    #[test]
    fn rename_changes_label_not_instance_owned_goal_or_selection() {
        let mut s = state();
        s.handle_goal(&fixture("active", 1));
        s.rename_session_rows("mob", "renamed", 42);
        s.session_name = "renamed".into();
        assert!(s.handle_goal(&fixture("paused", 2)));
        assert_eq!(s.agents[0].session(), "renamed");
    }

    #[test]
    fn foreign_spool_and_goal_snapshots_share_ownership_without_inheriting_a_recycled_pane() {
        let mut s = State {
            goal_layout: Layout::Inline,
            discover: true,
            session_name: "home".into(),
            ..Default::default()
        };
        let mut snapshot: Value = serde_json::from_str(&fixture("active", 1)).unwrap();
        snapshot["session"] = json!("foreign");
        let scan = format!("SERVER 42 foreign\nSCAN foreign 3 codex 42\nSPOOL /private/status/foreign.3:ts=100,pane_id=3,session=foreign,server_pid=42,agent_pid=77,server_started=Fri Oct 30 12:00:00 2026,tool=codex,status=done,session_id=qa-thread\nGOAL {snapshot}\nSCANEND\n");
        assert!(s.apply_scan_result(crate::discover::parse(&scan)));
        assert_eq!(s.agents[0].status, crate::status::Status::Done);
        assert_eq!(
            s.agents[0]
                .goal
                .as_ref()
                .unwrap()
                .snapshot
                .goal
                .as_ref()
                .unwrap()
                .status,
            Status::Active
        );
        let recycled = "SERVER 42 foreign\nSCAN foreign 3 codex 42\nSPOOL /private/status/foreign.3:ts=101,pane_id=3,session=foreign,server_pid=42,agent_pid=88,server_started=Fri Oct 30 12:00:00 2026,tool=codex,status=working,session_id=new-thread\nSCANEND\n";
        assert!(s.apply_scan_result(crate::discover::parse(recycled)));
        assert!(s.agents[0].goal.is_none());
        assert!(!s.handle_goal(&fixture("complete", 2)));
    }

    #[test]
    fn quiet_local_agents_poll_for_external_goal_controls() {
        let mut s = state();
        s.now = crate::SPOOL_POLL_INTERVAL;
        assert!(s.spool_poll_due());
        s.timer_running = false;
        s.arm_timer();
        assert!(s.timer_running);
        let owners: Value = serde_json::from_str(&s.goal_owners()).unwrap();
        assert_eq!(owners[0]["agent_session_id"], "qa-thread");
        s.goal_layout = Layout::Off;
        assert!(s.spool_poll_due(), "activity recovery is independent from goal display");
        assert!(s.goal_owners().is_empty());
        assert!(!s.handle_goal(&fixture("active", 1)));
        s.discover = false;
        assert!(!s.spool_poll_due());
    }

    #[test]
    fn reported_goals_still_age_when_discovery_is_disabled() {
        let mut s = state();
        s.discover = false;
        s.handle_goal(&fixture("active", 1));
        s.timer_running = false;
        s.arm_timer();
        assert!(s.timer_running);
        s.now = crate::STALE_AFTER + 1.0;
        s.on_tick();
        assert!(!s.scan_pending);
        assert!(s.agents[0].goal.as_ref().unwrap().label(s.now).contains("old data"));
        assert!(!s.agents[0].spinning());
    }

    #[test]
    fn objectives_and_goal_states_are_searchable_without_overriding_pane_labels() {
        let mut s = state();
        s.handle_goal(&fixture("blocked", 1));
        s.agents[0].pane_title = "User renamed task".into();
        s.find = Some(Find {
            query: "Disposable".into(),
            cursor: None,
        });
        assert_eq!(s.find_matches(), vec![0]);
        s.find.as_mut().unwrap().query = "blocked".into();
        assert_eq!(s.find_matches(), vec![0]);
        assert_eq!(s.agents[0].display_task(), "User renamed task");
    }

    #[test]
    fn goal_lines_and_inspector_fit_terminal_cells_and_keep_state_labels() {
        let mut s = state();
        let mut raw: Value = serde_json::from_str(&fixture("budget", 1)).unwrap();
        raw["goal"]["objective"] = json!("日本語 café 👩🏽‍💻 ".repeat(100));
        raw["goal"]["tokensUsed"] = json!(10);
        s.handle_goal(&raw.to_string());
        let o = s.agents[0].goal.as_ref().unwrap();
        for width in [0, 1, 12, 24, 40, 60, 80, 120, 200] {
            let line = item_text(&o.line(0.0, width, true));
            assert!(cells(&line) <= width, "{width}: {line}");
            for line in o.inspector(0.0, width) {
                assert!(cells(&item_text(&line)) <= width);
            }
            if width >= 40 {
                assert!(line.contains("goal: budget-limited"));
            }
        }
        assert_eq!(o.snapshot.goal.as_ref().unwrap().status, Status::BudgetLimited);
        assert!(item_text(&o.line(0.0, 200, true)).contains("budget reached"));
    }

    #[test]
    fn source_text_cannot_inject_terminal_controls() {
        let mut raw: Value = serde_json::from_str(&fixture("active", 1)).unwrap();
        raw["goal"]["objective"] =
            json!("safe\u{1b}[31mred\u{1b}[0m\n\u{1b}]8;;https://bad\u{7}link\u{1b}]8;;\u{1b}\\\u{202e}end");
        let parsed = Snapshot::parse(&raw.to_string()).unwrap();
        assert_eq!(parsed.goal.unwrap().objective, "safered linkend");
    }
}
