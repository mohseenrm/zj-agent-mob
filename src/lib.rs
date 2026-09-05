//! Zellij plugin that monitors Claude Code and Codex agents in the current session.

mod agent;
mod discover;
mod host;
mod install;
mod keys;
mod notify;
mod plugin;
mod ribbon;
mod state;
mod status;
mod style;
mod util;

pub use state::State;

/// Exposed so `tests/hook_e2e.rs` can diff it against the hook's own `tr`.
#[doc(hidden)]
pub fn sanitize_session_for_test(name: &str) -> String {
    agent::sanitize_session(name)
}

pub(crate) const SPINNER: [&str; 10] = [
    "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}", "\u{2827}", "\u{2807}",
    "\u{280f}",
];

pub(crate) const TICK: f64 = 0.25;

/// How long a foreign row's status is trusted before it decays to `unknown`.
pub(crate) const STALE_AFTER: f64 = 60.0;

/// How often the spool is re-read while any foreign agent is on screen. Well
/// inside `STALE_AFTER`, so a row gets many chances to refresh before it decays.
pub(crate) const SPOOL_POLL_INTERVAL: f64 = 5.0;

/// Shown in the pane frame instead of the full wasm path.
pub(crate) const PANE_TITLE: &str = "Agent Mob";

/// Longest reply the panel will compose. A one-line answer to a prompt, not a
/// prose channel: the panel truncates for display, so an uncapped buffer would
/// send far more than the line ever showed.
pub(crate) const MAX_REPLY_CHARS: usize = 200;

/// Ctrl-C. Interrupting a foreign pane goes through `zellij action write`,
/// which takes bytes rather than a signal: there is no cross-session form of
/// the plugin's own `send_sigint_to_pane_id`.
pub(crate) const SIGINT_BYTE: u8 = 3;

/// How long an agent may sit `waiting` before its row is painted as a fire
/// rather than a state. Long enough that a prompt you are actively answering
/// never escalates.
pub(crate) const WAITING_ESCALATE_AFTER: f64 = 120.0;

/// What the hook waits for a verdict by default, mirroring the hook's own
/// `ZJ_AGENT_APPROVE_TIMEOUT`. Only used for a prompt from a hook too old to
/// send its own: a newer one states the timeout it is actually using.
pub(crate) const DEFAULT_APPROVE_TIMEOUT: f64 = 30.0;

/// Stops a very wide pane from stretching a task summary across the screen.
pub(crate) const MAX_WIDTH: usize = 120;

/// The width every element lays out against, deliberately one column short of
/// the pane: a line that exactly fills it wraps and eats the row below.
pub(crate) fn content_width(cols: usize) -> usize {
    cols.saturating_sub(1).clamp(1, MAX_WIDTH)
}

/// A facade over `State` for the integration suite, which lives outside the
/// crate and so cannot reach `pub(crate)` internals. Mirrors what the panel
/// itself does rather than adding behaviour: every method here is a thin
/// wrapper over the same call the plugin's own event loop makes.
#[doc(hidden)]
pub mod testing {
    use crate::state::State;
    use std::collections::BTreeMap;
    use zellij_tile::prelude::{BareKey, KeyWithModifier};

    /// One spool record as a test writes it: session, pane, timestamp, fields.
    pub type Record<'a> = (&'a str, u32, f64, Vec<(&'a str, &'a str)>);

    pub fn key(c: char) -> KeyWithModifier {
        match c {
            '\r' | '\n' => KeyWithModifier::new(BareKey::Enter),
            '\x1b' => KeyWithModifier::new(BareKey::Esc),
            '\x08' => KeyWithModifier::new(BareKey::Backspace),
            c => KeyWithModifier::new(BareKey::Char(c)),
        }
    }

    pub struct Sim {
        state: State,
    }

    impl Sim {
        pub fn new(home: &str, sessions: &[&str]) -> Self {
            let state = State {
                permissions_granted: true,
                session_name: home.to_string(),
                live_sessions: sessions.iter().map(|s| s.to_string()).collect(),
                session_names: sessions.iter().map(|s| (s.to_string(), s.to_string())).collect(),
                ..Default::default()
            };
            Sim { state }
        }

        pub fn status(&mut self, args: &BTreeMap<String, String>) -> bool {
            self.state.handle_status(args)
        }

        pub fn ask(&mut self, args: &BTreeMap<String, String>) -> bool {
            self.state.handle_ask(args)
        }

        pub fn has_ask(&self, i: usize) -> bool {
            let id = &self.state.agents[i].id;
            self.state.ask_for(id).is_some()
        }

        pub fn press(&mut self, k: KeyWithModifier) -> bool {
            self.state.handle_key(k)
        }

        pub fn sessions(&mut self, live: &[&str]) -> bool {
            self.state.apply_sessions(live.iter().map(|s| s.to_string()).collect())
        }

        pub fn tick(&mut self) -> bool {
            self.state.on_tick()
        }

        pub fn select(&mut self, i: usize) {
            self.state.selected = i;
        }

        pub fn selected(&self) -> usize {
            self.state.selected
        }

        pub fn agent_count(&self) -> usize {
            self.state.agents.len()
        }

        pub fn agent_ids(&self) -> Vec<(String, u32)> {
            self.state
                .agents
                .iter()
                .map(|a| (a.id.session.clone(), a.id.pane_id))
                .collect()
        }

        pub fn ranks(&self) -> Vec<((String, u32), u8)> {
            self.state
                .agents
                .iter()
                .map(|a| ((a.id.session.clone(), a.id.pane_id), a.status.rank()))
                .collect()
        }

        pub fn status_of(&self, i: usize) -> &'static str {
            self.state.agents[i].status.label()
        }

        pub fn counters(&self) -> Vec<((String, u32), u32, u32, u32)> {
            self.state
                .agents
                .iter()
                .map(|a| {
                    (
                        (a.id.session.clone(), a.id.pane_id),
                        a.subagents,
                        a.tasks_total,
                        a.tasks_done,
                    )
                })
                .collect()
        }

        pub fn subagent_types(&self, i: usize) -> Vec<String> {
            self.state.agents[i].subagent_types.clone()
        }

        pub fn kill_armed(&self) -> Option<(String, u32)> {
            self.state.kill_armed.as_ref().map(|i| (i.session.clone(), i.pane_id))
        }

        pub fn reply_target(&self) -> Option<(String, u32)> {
            self.state.reply.as_ref().map(|r| (r.id.session.clone(), r.id.pane_id))
        }

        pub fn reply_text(&self) -> String {
            self.state.reply.as_ref().map(|r| r.text.clone()).unwrap_or_default()
        }

        pub fn head_line(&self, width: usize) -> String {
            self.state.head_line(width)
        }

        /// Puts the panel into one of its modal screens, by the same route a
        /// user takes: the install and setup screens own the whole screen, the
        /// reply editor is a text field, and a jump count swallows digits.
        pub fn enter_mode(&mut self, mode: &str) {
            match mode {
                "install" => {
                    self.state.install.open = true;
                }
                "setup" => {
                    let ctx: BTreeMap<String, String> = [(
                        crate::install::CTX_KEY.to_string(),
                        crate::install::CTX_STATUS.to_string(),
                    )]
                    .into_iter()
                    .collect();
                    self.state
                        .install
                        .on_command_result(Some(0), "claude=absent\ncodex=absent\n", "", &ctx);
                    self.state.agents.clear();
                }
                "reply" => {
                    self.state.begin_reply();
                }
                "jump" => {
                    self.state.handle_key(key('g'));
                }
                other => panic!("unknown mode {:?}", other),
            }
        }

        pub fn scan(&mut self, found: &[(&str, u32, &str)], spooled: &[Record<'_>]) -> bool {
            let found = found
                .iter()
                .map(|(s, p, t)| crate::discover::Found {
                    session: s.to_string(),
                    pane_id: *p,
                    tool: t.to_string(),
                })
                .collect();
            let spooled = spooled
                .iter()
                .map(|(s, p, ts, args)| crate::discover::Spooled {
                    session: s.to_string(),
                    pane_id: *p,
                    ts: *ts,
                    args: args.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
                })
                .collect();
            self.state.apply_scan_result(crate::discover::Scan {
                found,
                spooled,
                complete: true,
            })
        }

        pub fn parse_scan(&mut self, stdout: &str) -> bool {
            self.state.apply_scan_result(crate::discover::parse(stdout))
        }

        pub fn now(&self) -> f64 {
            self.state.now
        }

        pub fn clone_state(&self) -> Sim {
            let mut s = Sim::new(&self.state.session_name, &[]);
            s.state.live_sessions = self.state.live_sessions.clone();
            s.state.session_names = self.state.session_names.clone();
            for a in &self.state.agents {
                let args: BTreeMap<String, String> = [
                    ("session".to_string(), a.id.session.clone()),
                    ("pane_id".to_string(), a.id.pane_id.to_string()),
                    ("status".to_string(), a.status.label().replace("idle-wait", "idlewait")),
                    ("session_id".to_string(), a.session_id.clone()),
                    ("cwd".to_string(), a.cwd.clone()),
                ]
                .into_iter()
                .collect();
                s.state.handle_status(&args);
            }
            s.state.selected = self.state.selected;
            s
        }
    }
}
