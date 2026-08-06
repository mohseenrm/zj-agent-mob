// Zellij plugin entry point.
//
// The plugin logic lives in lib.rs so `cargo test` can exercise the state
// machine natively. `register_plugin!` must be invoked here, in the binary
// crate: it generates its own `fn main()` plus the `#[no_mangle]` load/update/
// render/pipe exports, and Zellij's loader needs the WASI `_start` entry point
// that only a bin target provides. A bare cdylib fails at load time with
// "could not find exported function".
use zellij_tile::prelude::*;
use zj_agent_mob::State;

register_plugin!(State);
