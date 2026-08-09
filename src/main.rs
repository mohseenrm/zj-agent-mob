// Must be a bin, not a cdylib: Zellij's loader needs the WASI `_start` export,
// and `register_plugin!` generates its own `fn main()`.
//
// `register_plugin!` pulls in `host_run_plugin_command`, a wasm import with no
// native definition, so off-wasm this is an empty main - otherwise anything
// that links the bin natively (any test under tests/) fails to build.
#[cfg(target_arch = "wasm32")]
use zellij_tile::prelude::*;
#[cfg(target_arch = "wasm32")]
use zj_agent_mob::State;

#[cfg(target_arch = "wasm32")]
register_plugin!(State);

#[cfg(not(target_arch = "wasm32"))]
fn main() {}
