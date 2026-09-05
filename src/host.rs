//! Host calls are WASM imports with no native symbol, so they no-op off-wasm to
//! keep everything else testable with a plain `cargo test`.

#[cfg(target_family = "wasm")]
pub(crate) use zellij_tile::shim::{
    close_terminal_pane, focus_terminal_pane, hide_self, open_command_pane_floating, run_command,
    send_sigint_to_pane_id, set_timeout, show_self, switch_session_with_focus, write_chars_to_pane_id,
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

#[cfg(target_family = "wasm")]
pub(crate) fn append_approve_rule(tool: &str) {
    let mut ctx = std::collections::BTreeMap::new();
    ctx.insert("kind".to_string(), "rule".to_string());
    run_command(
        &[
            "sh",
            "-c",
            "d=\"${ZJ_AGENT_APPROVE_RULES:-$HOME/.config/zj-agent-mob/approve.rules}\"; \
             mkdir -p \"$(dirname \"$d\")\" 2>/dev/null; \
             grep -qxF \"allow $1\" \"$d\" 2>/dev/null || printf 'allow %s\\n' \"$1\" >> \"$d\"",
            "sh",
            tool,
        ],
        ctx,
    );
}

#[cfg(target_family = "wasm")]
pub(crate) fn queue_followup(session: &str, pane_id: u32, text: &str) {
    let mut ctx = std::collections::BTreeMap::new();
    ctx.insert("kind".to_string(), "followup".to_string());
    run_command(
        &[
            "sh",
            "-c",
            "d=\"${TMPDIR:-/tmp}/zj-agent-mob\"; \
             [ -d \"$d\" ] || { mkdir -p \"$d\" 2>/dev/null && chmod 700 \"$d\" 2>/dev/null; }; \
             printf '%s' \"$3\" > \"$d/followup.$1.$2\" 2>/dev/null || true",
            "sh",
            session,
            &pane_id.to_string(),
            text,
        ],
        ctx,
    );
}

/// Fires a desktop notification through whichever notifier was detected.
///
/// The message carries task summaries and tool arguments, both of which come
/// from arbitrary repo content, so every one is its own argv element and none
/// is ever interpolated into a string a shell parses. `osascript` has no argv
/// form for `display notification`, so its text is bound to a variable via
/// `on run argv` instead of being spliced into the script.
#[cfg(target_family = "wasm")]
pub(crate) fn notify(notifier: &str, title: &str, body: &str, sound: bool) {
    let mut ctx = std::collections::BTreeMap::new();
    ctx.insert("kind".to_string(), "notify".to_string());
    match notifier {
        "osascript" => {
            let script = match sound {
                true => "on run argv\ndisplay notification (item 2 of argv) with title (item 1 of argv) sound name \"Ping\"\nend run",
                false => "on run argv\ndisplay notification (item 2 of argv) with title (item 1 of argv)\nend run",
            };
            run_command(&["osascript", "-e", script, title, body], ctx);
        }
        "terminal-notifier" => {
            run_command(
                &[
                    "terminal-notifier",
                    "-title",
                    title,
                    "-message",
                    body,
                    "-group",
                    "zj-agent-mob",
                ],
                ctx,
            );
        }
        "notify-send" => {
            run_command(&["notify-send", "-a", "zj-agent-mob", title, body], ctx);
        }
        _ => {}
    }
}

/// Acts on a pane in another Zellij session by shelling out to the `zellij`
/// binary, which takes a session argument where the plugin shims cannot.
/// Every value is its own argv element.
#[cfg(target_family = "wasm")]
pub(crate) fn session_action(session: &str, args: &[&str], kind: &str) {
    let mut ctx = std::collections::BTreeMap::new();
    ctx.insert("kind".to_string(), kind.to_string());
    let mut argv = vec!["zellij", "--session", session, "action"];
    argv.extend_from_slice(args);
    run_command(&argv, ctx);
}

/// Publishes the one-line fleet summary for status bars to render. `zellij pipe`
/// with no `--plugin` reaches every listening plugin, and the spool file serves
/// consumers that are not plugins at all.
#[cfg(target_family = "wasm")]
pub(crate) fn publish_summary(summary: &str, path: &str, kv: &str) {
    let mut ctx = std::collections::BTreeMap::new();
    ctx.insert("kind".to_string(), "summary".to_string());
    run_command(
        &[
            "sh",
            "-c",
            // The summary reaches a file and a pipe, so it is bound as a
            // positional rather than spliced into the command string.
            // The prose line and the `k=v` line are written as two files, both
            // atomically: a consumer reading mid-write would otherwise see a
            // truncated count and render it as fact.
            "printf '%s' \"$1\" > \"$2.tmp\" 2>/dev/null && mv -f \"$2.tmp\" \"$2\" 2>/dev/null; \
             printf '%s' \"$3\" > \"$2.kv.tmp\" 2>/dev/null && mv -f \"$2.kv.tmp\" \"$2.kv\" 2>/dev/null; \
             command -v zellij >/dev/null 2>&1 && zellij pipe --name zj-agent-mob-summary -- \"$1\" >/dev/null 2>&1 || true",
            "sh",
            summary,
            path,
            kv,
        ],
        ctx,
    );
}

#[cfg(not(target_family = "wasm"))]
mod stub {
    use std::collections::BTreeMap;
    use zellij_tile::prelude::{CommandToRun, FloatingPaneCoordinates, PaneId};
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
    pub(crate) fn append_approve_rule(_tool: &str) {}
    pub(crate) fn queue_followup(_session: &str, _pane_id: u32, _text: &str) {}
    pub(crate) fn notify(_notifier: &str, _title: &str, _body: &str, _sound: bool) {}
    pub(crate) fn session_action(_session: &str, _args: &[&str], _kind: &str) {}
    pub(crate) fn publish_summary(_summary: &str, _path: &str, _kv: &str) {}
    pub(crate) fn write_chars_to_pane_id(_chars: &str, _id: PaneId) {}
    pub(crate) fn open_command_pane_floating(
        _cmd: CommandToRun,
        _coords: Option<FloatingPaneCoordinates>,
        _ctx: BTreeMap<String, String>,
    ) -> Option<PaneId> {
        None
    }
}
#[cfg(not(target_family = "wasm"))]
pub(crate) use stub::*;
