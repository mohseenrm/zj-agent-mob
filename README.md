# Zellij Agent Mob (zj-agent-mob)

[![CI](https://github.com/mohseenrm/zj-agent-mob/actions/workflows/ci.yml/badge.svg)](https://github.com/mohseenrm/zj-agent-mob/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/mohseenrm/zj-agent-mob?sort=semver)](https://github.com/mohseenrm/zj-agent-mob/releases/latest)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
![Zellij 0.44+](https://img.shields.io/badge/zellij-0.44%2B-green.svg)

A Zellij plugin that monitors Claude Code and Codex agents running in your current session: live status, what each agent is working on, jump-to-pane, and ability to kill agents.

![The agent list: one row per agent with status, elapsed time, project, and task, each with an indented detail line](docs/img/02-agent-list.png)

## Contents

- [Requirements](#requirements)
- [Quick start](#quick-start)
- [Screens](#screens)
- [Keys](#keys)
- [Statuses](#statuses)
- [Known limitations](#known-limitations)
- Additional docs:
  - [Setup: install, Zellij configuration](docs/setup.md)
  - [Local development](docs/development.md)
  - [Troubleshooting](docs/troubleshooting.md)

## Requirements

| Requirement | Why |
|---|---|
| Zellij 0.44+ | Plugin API (`LaunchOrFocusPlugin`, pipes, `RunCommandResult`) |
| `jq` | The hook parses hook-event JSON; the installer merges settings |
| Claude Code, Codex, or both | The agents being monitored |
| Rust + `wasm32-wasip1` target | Only to build from source; not needed if you download a release |


## Quick start

### 1.A. Download the prebuilt plugin from the [latest release](https://github.com/mohseenrm/zj-agent-mob/releases/latest). 

Use `gh` for downloading prebuilt releases ([details](docs/setup.md#from-a-release)):

```bash
mkdir -p target/wasm32-wasip1/release
gh release download --repo mohseenrm/zj-agent-mob v0.1.0 \
  --pattern zj-agent-mob.wasm --dir target/wasm32-wasip1/release
./init.sh
```

### 1.B Or build it yourself:

```bash
rustup target add wasm32-wasip1
cargo build --release --target wasm32-wasip1
./init.sh
```

### 2. Add keybinding to `~/.config/zellij/config.kdl`:

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

## Screens

**First run.** If neither agent's hooks are installed, nothing can report status, so the panel offers to install them rather than sitting empty. Press <kbd>1</kbd>, <kbd>2</kbd>, or <kbd>3</kbd> to install without leaving Zellij.

![The setup screen listing four quick actions: install for Claude Code, for Codex, for both, or quit](docs/img/01-setup.png)

**Hooks installed, nothing running yet.** Start `claude` or `codex` in any pane and it appears here.

![The empty state telling you to start claude or codex in a pane](docs/img/00-empty.png)

**Killing an agent.** <kbd>x</kbd> sends an interrupt and arms the row; pressing it again closes the pane. The armed row says so in red, so the destructive step is never one keystroke away.

![The agent list with the selected row showing "press x again to close pane" in red](docs/img/03-kill-armed.png)

**The install screen** (<kbd>i</kbd>) toggles each target: pressing a row's key installs it when absent and uninstalls it when present.

![The install screen showing Claude Code hooks, Codex hooks, and Plugin wasm all installed](docs/img/04-install.png)

Targets are independent, so running only one agent's hooks is a supported state:

![The install screen with Claude Code and the plugin installed but Codex hooks absent](docs/img/05-install-partial.png)

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


## Known limitations

- Agents started before `init.sh` ran aren't tracked (no hooks installed yet). Restart them.
- Claude has no "permission granted" event, so `waiting` to `working` relies on the next tool-event heartbeat. With `ZJ_AGENT_HEARTBEAT=0`, `waiting` persists until the turn ends.

Hitting something not listed here? See [docs/troubleshooting.md](docs/troubleshooting.md).

## Releases

Prebuilt `zj-agent-mob.wasm` binaries and changelogs are on the [releases page](https://github.com/mohseenrm/zj-agent-mob/releases). Pushing a `v*` tag builds the wasm, verifies it exports what Zellij needs, and publishes it automatically.

## License

[Apache-2.0](LICENSE)
