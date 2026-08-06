# Zellij Agent Mob (zj-agent-mob)

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
- [Quick start](#quick-start)
- [Keys](#keys)
- [Statuses](#statuses)
- [Known limitations](#known-limitations)
- Full docs:
  - [Setup: install, Zellij registration, configuration](docs/setup.md)
  - [How it works](docs/how-it-works.md)
  - [Local development](docs/development.md)
  - [Troubleshooting](docs/troubleshooting.md)

## Requirements

| Requirement | Why |
|---|---|
| Zellij 0.44+ | Plugin API (`LaunchOrFocusPlugin`, pipes, `RunCommandResult`) |
| Rust + `wasm32-wasip1` target | Building the plugin |
| `jq` | The hook parses hook-event JSON; the installer merges settings |
| Claude Code and/or Codex | The agents being monitored |

## Quick start

```sh
rustup target add wasm32-wasip1
cargo build --release --target wasm32-wasip1
./init.sh
```

Then add a keybinding to `~/.config/zellij/config.kdl`:

```kdl
keybinds {
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

> [!IMPORTANT]
> Restart any running `claude` / `codex` sessions after installing. Hooks are read at session start, so existing sessions won't report status.

See [docs/setup.md](docs/setup.md) for per-target install, the in-panel install screen, layout registration, and every config knob.

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

Agents are sorted `waiting`, `done`, `working`, `idle`, so whatever needs you is always at the top.

## Known limitations

- Agents started before `init.sh` ran aren't tracked (no hooks installed yet). Restart them.
- Hooks-only: tools without a hooks system (aider, opencode) don't appear. There's no screen-scraping fallback.
- Subagents are not tracked separately; status is per pane.
- Claude has no "permission granted" event, so `waiting` to `working` relies on the next tool-event heartbeat. With `ZJ_AGENT_HEARTBEAT=0`, `waiting` persists until the turn ends.
- Stow/dotfiles users: `init.sh` resolves symlinks and writes through to the real file, so hooks land in your dotfiles repo and show up as a `git diff` to commit.
- The install screen needs Zellij's "Run commands" permission. Denying it leaves the rest of the panel fully functional; only that screen stops working.

Hitting something not listed here? See [docs/troubleshooting.md](docs/troubleshooting.md).

## License

[Apache-2.0](LICENSE)
