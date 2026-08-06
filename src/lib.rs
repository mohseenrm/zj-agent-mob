//! zj-agent-mob: a Zellij plugin that monitors Claude Code and Codex agents
//! running in the current session.
//!
//! Module layout:
//!
//! - [`plugin`] - Zellij lifecycle (`load`/`update`/`pipe`/`render`)
//! - [`state`]  - panel state machine: pipe handling, pane reconciliation
//! - [`keys`]   - keyboard handling: selection, jump-to-pane, kill
//! - [`agent`]  - one monitored agent, and how its row is built
//! - [`status`] - the four agent states and their presentation
//! - [`style`]  - raw ANSI constants (see the module docs for why not `Text`)
//! - [`util`]   - formatting helpers
//! - [`host`]   - Zellij host-call shim so everything above is host-testable
//!
//! `register_plugin!` lives in `main.rs`, not here: the macro generates its own
//! `fn main()`, and Zellij's loader needs the WASI `_start` export that only a
//! bin target provides.

mod agent;
mod host;
mod keys;
mod plugin;
mod state;
mod status;
mod style;
mod util;

pub use state::State;

/// Braille throbber frames for `working` agents.
pub(crate) const SPINNER: [&str; 10] = ["\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}", "\u{2827}", "\u{2807}", "\u{280f}"];

/// Throbber tick interval, in seconds.
pub(crate) const TICK: f64 = 0.25;
