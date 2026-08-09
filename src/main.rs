// Must be a bin, not a cdylib: Zellij's loader needs the WASI `_start` export,
// and `register_plugin!` generates its own `fn main()`.
//
// The plugin body only links against the wasm host: `register_plugin!` pulls in
// `host_run_plugin_command`, which is a Zellij-provided wasm import with no
// native definition. Off-wasm this target is an empty `main` so that building
// anything that forces the bin to link natively - notably any integration test
// under tests/, which makes Cargo build every bin target - still succeeds.
// The real build (`--target wasm32-wasip1`) is unaffected.
#[cfg(target_arch = "wasm32")]
use zellij_tile::prelude::*;
#[cfg(target_arch = "wasm32")]
use zj_agent_mob::State;

#[cfg(target_arch = "wasm32")]
register_plugin!(State);

#[cfg(not(target_arch = "wasm32"))]
fn main() {}
