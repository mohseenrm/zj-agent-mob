// Must be a bin, not a cdylib: Zellij's loader needs the WASI `_start` export,
// and `register_plugin!` generates its own `fn main()`.
use zellij_tile::prelude::*;
use zj_agent_mob::State;

register_plugin!(State);
