# zj-agent-mob

A Zellij plugin that monitors Claude Code and Codex agents running in your current session: live status, what each agent is working on, jump-to-pane, and kill.

```
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
 ↵ jump  1-9 quick  x kill  d dismiss  q hide
```

Status comes from real agent hooks, not screen scraping. The task summary is pulled from the agent's own transcript.

## Install

Requires Rust (with the `wasm32-wasip1` target), `jq`, and Zellij 0.44+.

```sh
rustup target add wasm32-wasip1
cargo build --release --target wasm32-wasip1
./init.sh
```

`init.sh` is idempotent: it installs the hook script, copies the plugin, and merges hook entries into `~/.claude/settings.json` and `~/.codex/hooks.json` without disturbing hooks you already have. Use `./init.sh --dry-run` to preview and `./init.sh uninstall` to remove exactly what it added.

Then register the plugin and a keybinding in `~/.config/zellij/config.kdl`:

```kdl
plugins {
    zj-agent-mob location="file:~/.config/zellij/plugins/zj-agent-mob.wasm" {
        popup_on_waiting true
    }
}

keybinds {
    // Ctrl s already enters Session mode; c opens the panel from there.
    session {
        bind "c" {
            LaunchOrFocusPlugin "zj-agent-mob" {
                floating true
                move_to_focused_tab true
            };
            SwitchToMode "Normal"
        }
    }
}
```

Press `Ctrl s` then `c` to open the panel. Verify the config with `zellij setup --check`.

Restart any running `claude` / `codex` sessions so they pick up the hooks.

## Keys

| Key | Action |
|---|---|
| `j` / `k`, `↓` / `↑` | Move selection |
| `Enter` | Jump to that agent's pane (works across tabs) and hide the panel |
| `1`-`9` | Jump straight to agent N |
| `x` | Send SIGINT to the agent; press again to close the pane |
| `d` | Dismiss a `done` badge |
| `q` / `Esc` | Hide the panel |

## Statuses

| Status | Meaning | Hook event |
|---|---|---|
| `working` | Processing a turn | `UserPromptSubmit`, refreshed by `PreToolUse`/`PostToolUse` |
| `waiting` | Needs you now (permission prompt / question) | `Notification` (Claude), `PermissionRequest` |
| `done` | Finished while you were elsewhere | `Stop` |
| `idle` | Session open, nothing new | `SessionStart`, or `done` after you visit the pane |

`waiting` and `done` are the ones worth surfacing. When an agent enters `waiting` and the panel is hidden, it pops up with that agent pre-selected, so it's one keypress from notification to agent.

Agents are sorted `waiting → done → working → idle`, so whatever needs you is always at the top.

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

## Development

```sh
cargo test                                      # 29 tests, no Zellij needed
cargo build --release --target wasm32-wasip1
cp target/wasm32-wasip1/release/zj-agent-mob.wasm ~/.config/zellij/plugins/
```

The crate builds a **bin** target (`src/main.rs`), not just a cdylib. Zellij's loader needs the WASI `_start` export, which only a bin provides; a bare cdylib fails at load with `could not find exported function`. `register_plugin!` also generates its own `fn main()`, so it must be invoked in `main.rs`. The lib target (`src/lib.rs`) holds all the logic purely so `cargo test` can run it natively.

To check a build has the right exports:

```sh
wasm-objdump -x zj-agent-mob.wasm | grep -A8 'Export\['   # want _start present
```

### Module layout

| File | Lines | Role |
|---|---|---|
| `main.rs` | 12 | `register_plugin!` + WASI entry point |
| `lib.rs` | 34 | Module wiring and shared constants |
| `plugin.rs` | 111 | Zellij lifecycle: permissions, subscriptions, `render` |
| `state.rs` | 206 | State machine: pipe handling, pane reconciliation |
| `keys.rs` | 95 | Keyboard: selection, jump-to-pane, two-step kill |
| `agent.rs` | 123 | One agent, and how its row is built |
| `status.rs` | 53 | The four states and their presentation |
| `style.rs` | 22 | ANSI constants |
| `util.rs` | 30 | `fmt_elapsed`, `truncate` |
| `host.rs` | 25 | Host-call shim |

Tests live beside the code they cover: 18 in `state.rs`, 9 in `agent.rs`, 2 in `util.rs`.

Zellij host calls (`focus_terminal_pane`, `hide_self`, ...) are WASM imports with no native symbol, so they're behind the `host` shim that no-ops off-wasm. That keeps the whole state machine and all layout code unit-testable with a plain `cargo test`.

**Zellij caches compiled plugins.** After rebuilding, a running session keeps using the old WASM, which makes changes look like they had no effect. Force a reload with `--skip-plugin-cache`:

```sh
zellij pipe --name agent-status --plugin file:...wasm --skip-plugin-cache --args "..."
```

or clear the cache (`~/Library/Caches/org.Zellij-Contributors.Zellij/` on macOS) and start a new session.

### Rendering note

The panel uses plain ANSI, not Zellij's `Text` / ribbon UI components. Those serialize to a DCS sequence that repositions the cursor itself, so consecutive components collapse onto one grid row. The cost is that colours are fixed 256-colour codes rather than following your Zellij theme.

If you do use the component API elsewhere, note that `Text::color_range()` indices are **byte** offsets (`serialize()` encodes via `as_bytes()`), not character offsets. Character counts corrupt the payload for multi-byte glyphs like `▶` or the braille spinner.

## Known limitations

- Agents started before `init.sh` ran aren't tracked (no hooks installed yet). Restart them.
- Hooks-only: tools without a hooks system (aider, opencode) don't appear. There's no screen-scraping fallback.
- Subagents are not tracked separately; status is per pane.
- Claude has no "permission granted" event, so `waiting → working` relies on the next tool-event heartbeat. With `ZJ_AGENT_HEARTBEAT=0`, `waiting` persists until the turn ends.
- Stow/dotfiles users: `init.sh` resolves symlinks and writes through to the real file, so hooks land in your dotfiles repo and show up as a `git diff` to commit.
