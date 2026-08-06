# zj-agent-mob

[![CI](https://github.com/mohseenrm/zj-agent-mob/actions/workflows/ci.yml/badge.svg)](https://github.com/mohseenrm/zj-agent-mob/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
![Zellij 0.44+](https://img.shields.io/badge/zellij-0.44%2B-green.svg)

A Zellij plugin that monitors Claude Code and Codex agents running in your current session: live status, what each agent is working on, jump-to-pane, and kill.

```text
zj-agent-mob   1 waiting · 1 working · 1 done
────────────────────────────────────────────────────────────────────────
▶ 1 ● codex   waiting  2m14s  web         Fix flaky checkout test
      └ needs approval: rm -rf node_modules · 3 turns · tab:1 · pane:5
  2 ✓ claude  done     5m01s  dotfiles    Review zellij plugin docs
      └ 12 turns · tab:1 · pane:2
  3 ⠋ claude  working     8s  api         Add retry to webhook client
      └ Edit src/webhook.rs · 47 turns · tab:2 · pane:3
  4 ○ codex   idle    15m00s  cli         Bump deps
      └ tab:2 · pane:7
────────────────────────────────────────────────────────────────────────
 ↵ jump  1-9 quick  x kill  d dismiss  i install  q hide
```

Status comes from real agent hooks, not screen scraping. The task summary is pulled from the agent's own transcript.

## Contents

- [Requirements](#requirements)
- [Install](#install)
- [Register the plugin with Zellij](#register-the-plugin-with-zellij)
- [Keys](#keys)
- [Statuses](#statuses)
- [Configuration](#configuration)
- [How it works](#how-it-works)
- [Local development](#local-development)
- [Troubleshooting](#troubleshooting)
- [Known limitations](#known-limitations)

## Requirements

| Requirement | Why |
|---|---|
| Zellij 0.44+ | Plugin API (`LaunchOrFocusPlugin`, pipes, `RunCommandResult`) |
| Rust + `wasm32-wasip1` target | Building the plugin |
| `jq` | The hook parses hook-event JSON; the installer merges settings |
| Claude Code and/or Codex | The agents being monitored |

## Install

```sh
rustup target add wasm32-wasip1
cargo build --release --target wasm32-wasip1
./init.sh
```

`init.sh` installs the hook script, copies the plugin, and merges hook entries into `~/.claude/settings.json` and `~/.codex/hooks.json` without disturbing hooks you already have. It is idempotent, so re-running it is safe.

```sh
./init.sh                  # install everything
./init.sh install claude   # just Claude Code's hooks
./init.sh install codex    # just Codex's hooks
./init.sh install plugin   # just copy the built wasm
./init.sh status           # what is currently installed
./init.sh --dry-run        # preview, write nothing
./init.sh uninstall        # remove exactly what was installed
./init.sh uninstall codex  # remove one target only
```

After the first run you can do all of this from inside the panel instead: press <kbd>i</kbd> for the install screen.

```text
zj-agent-mob   install
────────────────────────────────────────────────────────
▶ c  Claude Code hooks    ✓ installed
  x  Codex hooks          ○ not installed
  p  Plugin wasm          ✓ installed
────────────────────────────────────────────────────────
 c/x/p toggle  ↵ toggle  r refresh  esc back
```

Each row toggles: pressing its key installs when absent and uninstalls when present. The screen shells out to the copy of the installer that `init.sh` leaves at `~/.config/zj-agent-mob/install.sh`, so it works regardless of where you cloned the repo. This needs Zellij's "Run commands" permission, which the plugin requests on first load.

> [!IMPORTANT]
> Restart any running `claude` / `codex` sessions after installing. Hooks are read at session start, so existing sessions won't report status.

## Register the plugin with Zellij

`init.sh` copies the plugin to `~/.config/zellij/plugins/zj-agent-mob.wasm`, but Zellij still needs to know how to open it. Add a keybinding to `~/.config/zellij/config.kdl`:

```kdl
keybinds {
    // Ctrl s already enters Session mode; c opens the panel from there.
    session {
        bind "c" {
            LaunchOrFocusPlugin "file:~/.config/zellij/plugins/zj-agent-mob.wasm" {
                floating true
                move_to_focused_tab true
            }
            SwitchToMode "Normal"
        }
    }
}
```

Press <kbd>Ctrl</kbd>+<kbd>s</kbd> then <kbd>c</kbd> to open the panel.

To bind a single chord instead, put it in `shared_except` so it works from any mode:

```kdl
keybinds {
    shared_except "locked" {
        bind "Ctrl a" {
            LaunchOrFocusPlugin "file:~/.config/zellij/plugins/zj-agent-mob.wasm" {
                floating true
                move_to_focused_tab true
            }
        }
    }
}
```

Plugin configuration goes in the same block as the launch action:

```kdl
LaunchOrFocusPlugin "file:~/.config/zellij/plugins/zj-agent-mob.wasm" {
    floating true
    move_to_focused_tab true
    popup_on_waiting true
}
```

To have the panel present in a layout from the start, add it to `~/.config/zellij/layouts/default.kdl`:

```kdl
layout {
    pane size=1 borderless=true { plugin location="zellij:tab-bar"; }
    pane
    floating_panes {
        pane {
            plugin location="file:~/.config/zellij/plugins/zj-agent-mob.wasm"
            width "80%"
            height "50%"
        }
    }
}
```

Validate whatever you write with:

```sh
zellij setup --check
```

## Keys

### Agent list

| Key | Action |
|---|---|
| <kbd>j</kbd> / <kbd>k</kbd>, <kbd>↓</kbd> / <kbd>↑</kbd> | Move selection |
| <kbd>Enter</kbd> | Jump to that agent's pane (works across tabs) and hide the panel |
| <kbd>1</kbd>–<kbd>9</kbd> | Jump straight to agent N |
| <kbd>x</kbd> | Send SIGINT to the agent; press again to close the pane |
| <kbd>d</kbd> | Dismiss a `done` badge |
| <kbd>i</kbd> | Open the install screen |
| <kbd>q</kbd> / <kbd>Esc</kbd> | Hide the panel |

### Install screen

| Key | Action |
|---|---|
| <kbd>c</kbd> / <kbd>x</kbd> / <kbd>p</kbd> | Toggle Claude hooks / Codex hooks / the plugin wasm |
| <kbd>j</kbd> / <kbd>k</kbd>, <kbd>↓</kbd> / <kbd>↑</kbd> | Move selection |
| <kbd>Enter</kbd> | Toggle the selected row |
| <kbd>r</kbd> | Re-read install state |
| <kbd>i</kbd> / <kbd>q</kbd> / <kbd>Esc</kbd> | Back to the agent list |

## Statuses

| Status | Meaning | Hook event |
|---|---|---|
| `working` | Processing a turn | `UserPromptSubmit`, refreshed by `PreToolUse`/`PostToolUse` |
| `waiting` | Needs you now (permission prompt / question) | `Notification` (Claude), `PermissionRequest` (Codex) |
| `done` | Finished while you were elsewhere | `Stop` |
| `idle` | Session open, nothing new | `SessionStart`, or `done` after you visit the pane |

`waiting` and `done` are the ones worth surfacing. When an agent enters `waiting` and the panel is hidden, it pops up with that agent pre-selected, so it's one keypress from notification to agent.

Agents are sorted `waiting → done → working → idle`, so whatever needs you is always at the top.

## Configuration

Plugin config (in the layout or keybinding block):

| Key | Default | Meaning |
|---|---|---|
| `popup_on_waiting` | `true` | Auto-show the panel when an agent needs input |

Hook script environment:

| Variable | Default | Meaning |
|---|---|---|
| `ZJ_AGENT_TOOL` | `claude` | Which transcript reader to use (`claude` / `codex`) |
| `ZJ_AGENT_HEARTBEAT` | `1` | Set `0` to skip `PreToolUse`/`PostToolUse` (halves hook volume) |
| `ZJ_AGENT_PLUGIN` | `file:~/.config/zellij/plugins/zj-agent-mob.wasm` | Plugin path |
| `ZJ_AGENT_DEBUG` | `0` | Set `1` to log events to `~/.cache/zj-agent-mob/hook.log` |

Installer environment (mostly useful for testing against throwaway directories):

| Variable | Default |
|---|---|
| `ZJ_AGENT_HOOK_DIR` | `~/.config/zj-agent-mob` |
| `ZJ_AGENT_PLUGIN_DIR` | `~/.config/zellij/plugins` |
| `CLAUDE_CONFIG_DIR` | `~/.claude` |
| `CODEX_HOME` | `~/.codex` |

## How it works

Each hook event runs `scripts/zj-agent-mob-hook.sh`, which forwards status to the plugin:

```sh
zellij pipe --name agent-status --plugin file:...wasm \
  --args "pane_id=$ZELLIJ_PANE_ID,tool=claude,status=waiting,..."
```

`$ZELLIJ_PANE_ID` is set by Zellij in every terminal pane and is inherited by processes started there, so it identifies the exact pane. If it isn't set the hook exits immediately, which is what scopes monitoring to the current session: agents outside Zellij are ignored, and each session's plugin instance only sees its own panes.

`zellij pipe --plugin` auto-launches the plugin if it isn't running, so there's no daemon, socket, or state file.

### Task summaries

- **Claude Code** transcripts contain an `ai-title` record: Claude's own rolling summary of the session (e.g. "Review Zellij plugin UI rendering documentation"). Falls back to `last-prompt`, then the pane title.
- **Codex** has no equivalent, so the first `event_msg` / `user_message` record from the session rollout is used. Raw `response_item` records are skipped because they're padded with `AGENTS.md` and `<INSTRUCTIONS>` preamble.

Transcripts reach tens of megabytes, so summaries are only re-read on turn boundaries (`SessionStart`, `UserPromptSubmit`, `Stop`) from a bounded `tail -n 300` window. Tool events send an empty `task=`, and the plugin treats empty as "leave unchanged".

`/goal` sets a session-scoped hook but leaves no on-disk artifact, so goals aren't readable. `ai-title` tracks the real work anyway. For a manual override:

```sh
zellij pipe --name agent-label --args "pane_id=3,label=whatever you want"
```

### The install screen

The plugin runs in WASI with no access to `$HOME`, so it cannot read `settings.json` to see whether hooks are installed. Instead it shells out via Zellij's `run_command` to `~/.config/zj-agent-mob/install.sh status`, which prints one `key=state` line per target:

```text
claude=installed
codex=absent
plugin=installed
hook=installed
```

Results arrive asynchronously as a `RunCommandResult` event, tagged with a context key so install output is never confused with another command's. After any toggle the plugin re-reads state rather than assuming the change landed, because the installer can succeed partially (hooks written, plugin not built).

## Local development

```sh
git clone git@github.com:mohseenrm/zj-agent-mob.git
cd zj-agent-mob
rustup target add wasm32-wasip1
cargo test
```

The full check set, matching what CI runs:

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo build --release --target wasm32-wasip1
shellcheck --shell=sh init.sh scripts/zj-agent-mob-hook.sh
```

To iterate on the plugin against a live session:

```sh
cargo build --release --target wasm32-wasip1
./init.sh install plugin

# Zellij caches compiled plugins, so force a reload or the old build stays live.
zellij action launch-or-focus-plugin --skip-plugin-cache --floating \
  "file:$HOME/.config/zellij/plugins/zj-agent-mob.wasm"
```

To feed the panel a status without running a real agent:

```sh
zellij pipe --name agent-status \
  --plugin "file:$HOME/.config/zellij/plugins/zj-agent-mob.wasm" \
  --args "pane_id=$ZELLIJ_PANE_ID,tool=claude,status=waiting,task=manual test"
```

To test the installer without touching your real config, point it at throwaway directories:

```sh
export ZJ_AGENT_HOOK_DIR=/tmp/zj/hooks ZJ_AGENT_PLUGIN_DIR=/tmp/zj/plugins
export CLAUDE_CONFIG_DIR=/tmp/zj/claude CODEX_HOME=/tmp/zj/codex
./init.sh install && ./init.sh status && ./init.sh uninstall
```

### Why a bin target

The crate builds a **bin** target (`src/main.rs`), not just a cdylib. Zellij's loader needs the WASI `_start` export, which only a bin provides; a bare cdylib fails at load with `could not find exported function`. `register_plugin!` also generates its own `fn main()`, so it must be invoked in `main.rs`. The lib target (`src/lib.rs`) holds all the logic purely so `cargo test` can run it natively.

To check a build has the right exports:

```sh
wasm-objdump -x target/wasm32-wasip1/release/zj-agent-mob.wasm | grep -A8 'Export\['
```

You want `_start`, `load`, `update`, `render`, `pipe`, and `plugin_version`. CI asserts all six.

### Module layout

| File | Lines | Role |
|---|---|---|
| `main.rs` | 6 | `register_plugin!` + WASI entry point |
| `lib.rs` | 20 | Module wiring and shared constants |
| `plugin.rs` | 137 | Zellij lifecycle: permissions, subscriptions, `render` |
| `state.rs` | 187 | State machine: pipe handling, pane reconciliation |
| `install.rs` | 271 | Install screen: state, toggles, installer output parsing |
| `keys.rs` | 143 | Keyboard: selection, jump-to-pane, two-step kill |
| `agent.rs` | 111 | One agent, and how its row is built |
| `status.rs` | 53 | The four states and their presentation |
| `util.rs` | 30 | `fmt_elapsed`, `truncate` |
| `host.rs` | 22 | Host-call shim |
| `style.rs` | 12 | ANSI constants |

Line counts exclude tests. Tests live beside the code they cover: 18 in `state.rs`, 10 in `install.rs`, 9 in `agent.rs`, 6 in `keys.rs`, 2 in `util.rs` — 45 total, none needing a running Zellij.

Zellij host calls (`focus_terminal_pane`, `hide_self`, `run_command`, ...) are WASM imports with no native symbol, so they're behind the `host` shim that no-ops off-wasm. That keeps the whole state machine and all layout code unit-testable with a plain `cargo test`.

### Rendering note

The panel uses plain ANSI, not Zellij's `Text` / ribbon UI components. Those serialize to a DCS sequence that repositions the cursor itself, so consecutive components collapse onto one grid row. The cost is that colours are fixed 256-colour codes rather than following your Zellij theme.

If you do use the component API elsewhere, note that `Text::color_range()` indices are **byte** offsets (`serialize()` encodes via `as_bytes()`), not character offsets. Character counts corrupt the payload for multi-byte glyphs like `▶` or the braille spinner.

## Troubleshooting

### Changes to the plugin seem to have no effect

**Zellij caches compiled plugins.** After rebuilding, a running session keeps using the old WASM. Force a reload with `--skip-plugin-cache`, which lives on `launch-or-focus-plugin` (note: *not* on `zellij pipe`, which has no such flag):

```sh
zellij action launch-or-focus-plugin --skip-plugin-cache --floating \
  "file:$HOME/.config/zellij/plugins/zj-agent-mob.wasm"
```

Or clear the cache and start a new session:

```sh
# macOS
rm -rf ~/Library/Caches/org.Zellij-Contributors.Zellij/
# Linux
rm -rf ~/.cache/zellij/
```

### The panel says "no agents in this session"

Work through these in order:

1. **Are the hooks installed?** Press <kbd>i</kbd>, or run `./init.sh status`.
2. **Did you restart the agent?** Hooks are read at session start. An agent that was already running when you installed them reports nothing.
3. **Is the agent inside Zellij?** The hook exits immediately when `$ZELLIJ_PANE_ID` is unset. Check with `echo $ZELLIJ_PANE_ID` in the agent's pane.
4. **Is the hook firing at all?** Turn on logging and watch it:

   ```sh
   export ZJ_AGENT_DEBUG=1
   tail -f ~/.cache/zj-agent-mob/hook.log
   ```

   No lines means the agent isn't invoking the hook; lines but no panel update means the `zellij pipe` call is failing.

5. **Can you drive the panel by hand?** This bypasses the agent entirely:

   ```sh
   zellij pipe --name agent-status \
     --plugin file:~/.config/zellij/plugins/zj-agent-mob.wasm \
     --args "pane_id=$ZELLIJ_PANE_ID,tool=claude,status=waiting,task=manual test"
   ```

### The install screen says "Installer not found"

The plugin drives `~/.config/zj-agent-mob/install.sh`, which `init.sh` puts there by copying itself. Run `./init.sh` from the repo once to bootstrap it.

### The install screen shows `?` / "unknown" for everything

The status command failed. Common causes: `jq` isn't installed, or the plugin wasn't granted Zellij's "Run commands" permission. Press <kbd>r</kbd> to retry; a real error message is shown under the rows when there is one.

### Zellij fails to load the plugin

```text
could not find exported function
```

The wasm was built as a cdylib rather than a bin. Confirm the artifact you installed is `zj-agent-mob.wasm` (hyphens, from the bin target), not `zj_agent_mob.wasm` (underscores, the cdylib), then rebuild:

```sh
cargo build --release --target wasm32-wasip1
./init.sh install plugin
```

### `waiting` stays on screen after you've answered

Claude has no "permission granted" event, so `waiting → working` relies on the next tool-event heartbeat. If you set `ZJ_AGENT_HEARTBEAT=0`, `waiting` persists until the turn ends. That's the tradeoff for halving hook volume.

### Hooks landed in my dotfiles repo

That's intended. `init.sh` resolves symlinks and writes through to the real file, so a stow-managed `~/.claude/settings.json` gets the change in your dotfiles repo, where you can commit it. The installer prints a note when it detects this.

### The panel is cramped or columns are missing

The layout degrades by width: the project column is dropped under 50 columns, and the per-agent detail line needs at least 60 columns plus two rows per agent. Resize the floating pane, or set a larger `width` / `height` in the layout.

## Known limitations

- Agents started before `init.sh` ran aren't tracked (no hooks installed yet). Restart them.
- Hooks-only: tools without a hooks system (aider, opencode) don't appear. There's no screen-scraping fallback.
- Subagents are not tracked separately; status is per pane.
- Claude has no "permission granted" event, so `waiting → working` relies on the next tool-event heartbeat. With `ZJ_AGENT_HEARTBEAT=0`, `waiting` persists until the turn ends.
- Stow/dotfiles users: `init.sh` resolves symlinks and writes through to the real file, so hooks land in your dotfiles repo and show up as a `git diff` to commit.
- The install screen needs Zellij's "Run commands" permission. Denying it leaves the rest of the panel fully functional; only that screen stops working.

## License

[Apache-2.0](LICENSE)
