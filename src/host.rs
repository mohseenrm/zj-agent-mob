//! Host calls are WASM imports with no native symbol, so they no-op off-wasm to
//! keep everything else testable with a plain `cargo test`.

#[cfg(target_family = "wasm")]
pub(crate) use zellij_tile::shim::{
    close_terminal_pane, focus_terminal_pane, hide_self, send_sigint_to_pane_id, set_timeout,
    show_self,
};

#[cfg(not(target_family = "wasm"))]
mod stub {
    use zellij_tile::prelude::PaneId;
    pub(crate) fn set_timeout(_secs: f64) {}
    pub(crate) fn show_self(_float: bool) {}
    pub(crate) fn hide_self() {}
    pub(crate) fn focus_terminal_pane(_id: u32, _float: bool, _in_place: bool) {}
    pub(crate) fn close_terminal_pane(_id: u32) {}
    pub(crate) fn send_sigint_to_pane_id(_id: PaneId) {}
}
#[cfg(not(target_family = "wasm"))]
pub(crate) use stub::*;
