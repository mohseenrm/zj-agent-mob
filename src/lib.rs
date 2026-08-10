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
/// Nothing refreshes it: the hook only pipes into its own session.
pub(crate) const STALE_AFTER: f64 = 60.0;

/// Shown in the pane frame instead of the full wasm path.
pub(crate) const PANE_TITLE: &str = "Agent Mob";

/// Ctrl-C. Interrupting a foreign pane goes through `zellij action write`,
/// which takes bytes rather than a signal: there is no cross-session form of
/// the plugin's own `send_sigint_to_pane_id`.
pub(crate) const SIGINT_BYTE: u8 = 3;

/// How long an agent may sit `waiting` before its row is painted as a fire
/// rather than a state. Long enough that a prompt you are actively answering
/// never escalates.
pub(crate) const WAITING_ESCALATE_AFTER: f64 = 120.0;

/// Stops a very wide pane from stretching a task summary across the screen.
pub(crate) const MAX_WIDTH: usize = 120;

/// The width every element lays out against, deliberately one column short of
/// the pane: a line that exactly fills it wraps and eats the row below.
pub(crate) fn content_width(cols: usize) -> usize {
    cols.saturating_sub(1).clamp(1, MAX_WIDTH)
}
