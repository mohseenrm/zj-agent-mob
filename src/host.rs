//! Host calls are WASM imports with no native symbol, so they no-op off-wasm to
//! keep everything else testable with a plain `cargo test`.

#[cfg(target_family = "wasm")]
pub(crate) use zellij_tile::shim::{
    close_terminal_pane, focus_terminal_pane, hide_self, run_command, send_sigint_to_pane_id, set_timeout, show_self,
    switch_session_with_focus,
};

/// Renames the pane this plugin is running in. Needs our own plugin id, which
/// only the host can tell us, so it is a single call rather than two.
#[cfg(target_family = "wasm")]
pub(crate) fn rename_own_pane(title: &str) {
    let ids = zellij_tile::shim::get_plugin_ids();
    zellij_tile::shim::rename_plugin_pane(ids.plugin_id, title);
}

/// Drops a permission verdict where the blocked hook is polling for it. The
/// plugin's WASI sandbox has no access to the host filesystem, so this shells
/// out rather than writing directly.
/// The verdict is one of two fixed literals and the path is passed as its own
/// argv element, so nothing user-influenced is ever parsed by a shell.
#[cfg(target_family = "wasm")]
pub(crate) fn write_verdict(path: &str, verdict: &str) {
    let mut ctx = std::collections::BTreeMap::new();
    ctx.insert("kind".to_string(), "verdict".to_string());
    // `cp` from /dev/stdin needs a pipe we do not have; `sh -c` with the path as
    // a positional arg keeps it out of the parsed command string.
    run_command(&["sh", "-c", "printf '%s' \"$1\" > \"$2\"", "sh", verdict, path], ctx);
}

#[cfg(not(target_family = "wasm"))]
mod stub {
    use std::collections::BTreeMap;
    use zellij_tile::prelude::PaneId;
    pub(crate) fn set_timeout(_secs: f64) {}
    pub(crate) fn show_self(_float: bool) {}
    pub(crate) fn hide_self() {}
    pub(crate) fn focus_terminal_pane(_id: u32, _float: bool, _in_place: bool) {}
    pub(crate) fn close_terminal_pane(_id: u32) {}
    pub(crate) fn send_sigint_to_pane_id(_id: PaneId) {}
    pub(crate) fn run_command(_cmd: &[&str], _ctx: BTreeMap<String, String>) {}
    pub(crate) fn switch_session_with_focus(_name: &str, _tab: Option<usize>, _pane: Option<(u32, bool)>) {}
    pub(crate) fn rename_own_pane(_title: &str) {}
    pub(crate) fn write_verdict(_path: &str, _verdict: &str) {}
}
#[cfg(not(target_family = "wasm"))]
pub(crate) use stub::*;
