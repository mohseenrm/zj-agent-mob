//! Zellij plugin that monitors Claude Code and Codex agents in the current session.

mod agent;
mod host;
mod install;
mod keys;
mod plugin;
mod ribbon;
mod state;
mod status;
mod style;
mod util;

pub use state::State;

pub(crate) const SPINNER: [&str; 10] = [
    "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}", "\u{2827}", "\u{2807}",
    "\u{280f}",
];

pub(crate) const TICK: f64 = 0.25;

/// Shown in the pane frame instead of the full wasm path.
pub(crate) const PANE_TITLE: &str = "Agent Mob";

/// Rows stop growing past this, so a very wide pane doesn't stretch a task
/// summary across the whole screen. Rules and rows both use it, which is what
/// keeps them flush with each other.
pub(crate) const MAX_WIDTH: usize = 120;

/// The width every element in the panel lays out against.
///
/// One column short of the pane on purpose. A line that exactly fills the pane
/// wraps when its trailing newline lands, so the grid swallows a row and every
/// element below it renders one row too high - the detail line collides with
/// the row above it and the footer lands on the closing rule.
pub(crate) fn content_width(cols: usize) -> usize {
    cols.saturating_sub(1).clamp(1, MAX_WIDTH)
}
