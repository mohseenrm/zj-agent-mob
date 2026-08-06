//! Zellij plugin that monitors Claude Code and Codex agents in the current session.

mod agent;
mod host;
mod keys;
mod plugin;
mod state;
mod status;
mod style;
mod util;

pub use state::State;

pub(crate) const SPINNER: [&str; 10] = [
    "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}", "\u{2827}",
    "\u{2807}", "\u{280f}",
];

pub(crate) const TICK: f64 = 0.25;
