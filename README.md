# Zellij Agent Mob (zj-agent-mob)

[![CI](https://github.com/mohseenrm/zj-agent-mob/actions/workflows/ci.yml/badge.svg)](https://github.com/mohseenrm/zj-agent-mob/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/mohseenrm/zj-agent-mob?sort=semver)](https://github.com/mohseenrm/zj-agent-mob/releases/latest)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
![Zellij 0.44+](https://img.shields.io/badge/zellij-0.44%2B-green.svg)

**Keep track of every coding agent you have running, from one floating panel.**

Run enough Claude Code and Codex agents and they scatter across panes, tabs, and whole Zellij sessions. This panel shows all of them at once: who is working, who is blocked waiting on you, and who finished while you were elsewhere. Press <kbd>Enter</kbd> to jump straight to any of them, even in another session.

![Four agents appear across three Zellij sessions, a permission prompt is approved from the panel, statuses move through compact, failed and done, then the kill confirm, the install screen, and finally Enter jumps into an agent in another session](demo/tour.gif)

A blocked agent is invisible until you happen to cycle past its pane. Rows sort
by urgency, so whatever needs you most sits at the top.

- **Every agent at once**, across sessions, with live status and the task each is on.
- **Jump to any pane** with <kbd>Enter</kbd>, across tabs *and* sessions.
- **Fuzzy find** with <kbd>/</kbd>: a few characters of a task, worktree, or
  session narrows the list to it.
- **Told when you are away**: a desktop notification the moment an agent blocks or fails.
- **Answer in place**: <kbd>a</kbd> / <kbd>r</kbd> for permission prompts, <kbd>A</kbd> to always allow a tool,
  <kbd>y</kbd> / <kbd>m</kbd> to reply.
- **Kill a runaway** with <kbd>x</kbd>, two-step so you never do it by accident.
- **No daemon and no socket.** Hooks pipe straight to the plugin and drop a
  disposable status file for other sessions to read.

> Inspired by [herdr](https://herdr.dev), without adopting an entire new multiplexer.

**Docs:** [setup](docs/setup.md) ·
[how it works](docs/how-it-works.md) ·
[development](docs/development.md) ·
[troubleshooting](docs/troubleshooting.md)

## Requirements

| Requirement | Why |
|---|---|
| Zellij 0.44+ | Plugin API (`LaunchOrFocusPlugin`, pipes, `RunCommandResult`) |
| `jq` | The hook parses hook-event JSON; the installer merges settings |
| `curl` or `wget` | Only for the one-line install; not needed from a clone |
| Claude Code, Codex, or both | The agents being monitored |
| Rust + `wasm32-wasip1` target | Only to build from source; not needed if you download a release |

## Quick start

Three steps: install, bind a key, restart your agents.

### 1. Install

No clone and no Rust toolchain required:

```sh
curl -fsSL https://github.com/mohseenrm/zj-agent-mob/releases/download/v0.9.0/init.sh | sh
```

This downloads the plugin and hook script for that release, wires up whichever of Claude Code and Codex you have, and leaves an installer at `~/.config/zj-agent-mob/install.sh` so the in-panel install screen works from then on.

Prefer to read before running? Same thing in two steps:

```sh
curl -fsSL -O https://github.com/mohseenrm/zj-agent-mob/releases/download/v0.9.0/init.sh
less init.sh && sh init.sh
```

<details>
<summary>Or build it from source</summary>

```sh
rustup target add wasm32-wasip1
cargo build --release --target wasm32-wasip1
./init.sh
```

From a clone, `./init.sh` uses your local build and downloads nothing.

Reinstalling over an existing install, or syncing a checkout across machines? Use
`./scripts/reinstall-local.sh`, which also clears Zellij's plugin cache and any stale
status records - both of which survive a plain `init.sh` and keep old behaviour live.
`--check` reports whether what is installed matches your checkout.

</details>

### 2. Bind a key

Add to `~/.config/zellij/config.kdl`:

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

### 3. Restart your agents

> [!IMPORTANT]
> Restart any running `claude` / `codex` sessions after installing. Hooks are read at session start, so existing sessions won't report status.

Then start an agent in any pane and open the panel: it shows up within a turn. If the panel stays empty, [troubleshooting](docs/troubleshooting.md#the-panel-says-no-agents-in-this-session) walks through it in order.

See [docs/setup.md](docs/setup.md) for per-target install, the in-panel install screen, layout registration, and every config knob.

## Screens

<table>
<tr>
<td width="50%">

**The agent list.** One row per agent: status, elapsed time, project, task, and
an indented detail line.

![The agent list: one row per agent with status, elapsed time, project, and task](docs/img/02-agent-list.png)

</td>
<td width="50%">

**First run.** With no hooks installed nothing can report, so the panel offers
to install rather than sitting empty.

![The setup screen listing four quick actions: install for Claude Code, for Codex, for both, or quit](docs/img/01-setup.png)

</td>
</tr>
<tr>
<td width="50%">

**Killing an agent.** <kbd>x</kbd> interrupts and arms the row; again closes the
pane.

![The agent list with the selected row showing "press x again to close pane" in red](docs/img/03-kill-armed.png)

</td>
<td width="50%">

**The install screen** (<kbd>i</kbd>). Each target toggles independently, so
running only one agent's hooks is supported.

![The install screen showing Claude Code hooks, Codex hooks, and Plugin wasm all installed](docs/img/04-install.png)

</td>
</tr>
</table>

## Keys

### Agent list

| Key | Action |
|---|---|
| <kbd>j</kbd> / <kbd>k</kbd>, <kbd>↓</kbd> / <kbd>↑</kbd> | Move selection |
| <kbd>Enter</kbd> | Jump to that agent's pane (across tabs *and* sessions) and hide the panel |
| <kbd>1</kbd>–<kbd>9</kbd> | Jump straight to agent N |
| <kbd>g</kbd> <var>N</var> <kbd>Enter</kbd> | Jump to any row by number, including past 9. <kbd>g</kbd> opens a count, <kbd>Enter</kbd> or <kbd>G</kbd> closes it: `g25`<kbd>Enter</kbd>, or `g25G` for the vim spelling |
| <kbd>g</kbd><kbd>g</kbd> / <kbd>G</kbd> | First row / last row |
| <kbd>/</kbd> | Fuzzy find: type to narrow the list (task, worktree, path, session, tool, status), <kbd>Ctrl</kbd>+<kbd>j</kbd>/<kbd>k</kbd> or <kbd>↓</kbd>/<kbd>↑</kbd> to pick a match, <kbd>Enter</kbd> jumps to it, <kbd>Esc</kbd> cancels. Smartcase, like vim |
| <kbd>s</kbd> | Cycle the ordering: urgency (default) -> grouped by project -> grouped by session |
| <kbd>x</kbd> | Send SIGINT to the agent; press again to close the pane (any session) |
| <kbd>a</kbd> / <kbd>r</kbd> | Approve / reject a parked permission prompt |
| <kbd>A</kbd> | Approve, and stop that tool asking again |
| <kbd>f</kbd> | Queue a follow-up, delivered when the turn ends |
| <kbd>y</kbd> | Answer a blocked agent with `y` (only shown while it is waiting) |
| <kbd>m</kbd> | Type a one-line reply to a blocked agent; <kbd>Enter</kbd> sends, <kbd>Esc</kbd> cancels |
| <kbd>d</kbd> | Dismiss a `done` badge |
| <kbd>D</kbd> | Dismiss every `done` badge at once |
| <kbd>n</kbd> | Open a new agent in a floating pane, in the selected row's directory (advertised in the empty state rather than the footer) |
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
| `failed` | Stopped by an error (rate limit, billing, auth) | `StopFailure` (Claude only) |
| `waiting` | Needs you now (permission prompt / question) | `Notification` (Claude), `PermissionRequest` (both) |
| `idle-wait` | Has been waiting on you for a while | `Notification` / `idle_prompt` (Claude only) |
| `done` | Finished while you were elsewhere | `Stop` |
| `compact` | Compacting context (looks like a hang otherwise) | `PreCompact`, cleared by `PostCompact` |
| `working` | Processing a turn | `UserPromptSubmit`, refreshed by `PreToolUse`/`PostToolUse` |
| `idle` | Session open, nothing new | `SessionStart`, or `done` after you visit the pane |
| `found` | Spotted by the process scan, but it has never fired a hook | - |
| `unknown` | Running, but nothing has reported on it in a while ([stuck there?](docs/troubleshooting.md#a-row-in-another-session-says-unknown)) | - |
| `gone` | Its Zellij session is gone, so its state is unknowable | - |

Rows sort in that order, so whatever needs you most is at the top. A `found` row is normal rather than broken: the agent was already running when hooks were installed, and it fills in the moment it next does anything.

### Ordering and grouping

<kbd>s</kbd> cycles the ordering: urgency (default), grouped by project, then
grouped by session. Each group is ranked by its **most urgent member**, so
grouping never buries a blocked agent under a quiet project. The active mode
shows in the header; the default needs no announcing.

```
zj-agent-mob   1 waiting · 2 working   [project groups · s]
────────────────────────────────────────────────────────────
  api (2)
▶ 1 ● claude  waiting    12s  api        Fix the failing auth test
      └ wants: permission · needs approval: git push --force · pane:6
  2 ⠙ claude  working     41s  api        Port the hook suite to Rust
  web (1)
  3 ⠙ codex   working   2m10s  web        Update the checkout flow
```

### Why an agent is blocked

A `waiting` row says what kind of answer it wants, so you can triage without
visiting a pane. Only a `permission` is a yes/no the panel can answer:

| `wants:` | You can |
|---|---|
| `permission` | Answer with <kbd>a</kbd> / <kbd>r</kbd>, or <kbd>A</kbd> to always allow that tool |
| `plan` | Read it - <kbd>Enter</kbd> to the pane |
| `question` | Reply with <kbd>m</kbd>, or jump to the pane |
| `idle` | Nothing is blocked on a decision |

### Answering permission prompts from the panel

On by default; `ZJ_AGENT_APPROVE=0` in the agent's environment opts out. A permission prompt
parks in the panel and <kbd>a</kbd> / <kbd>r</kbd> approve or reject it without leaving the panel:

```
▶ 1 ● codex   waiting    2s  web        Fix flaky checkout test
      └ needs approval: rm -rf node_modules · pane:5
        ┌──────────────────────────────────────────┐
        │ Bash                                     │
        │ rm -rf node_modules                      │
        │ a approve  r reject  A always  ↵ pane   │
        └──────────────────────────────────────────┘
```

It waits `ZJ_AGENT_APPROVE_TIMEOUT` seconds (default 30) and then falls through to the agent's own
prompt, so the worst case is the normal interactive experience. Reject is <kbd>r</kbd>, not
<kbd>d</kbd>, so a mis-keyed dismiss can never answer a prompt.

<kbd>A</kbd> approves *and* stops that tool asking again, by appending a line to
`~/.config/zj-agent-mob/approve.rules`:

```
allow Read
allow Bash git
```

A matching rule is answered immediately, with no prompt and no wait, so a fleet only interrupts
you for something new. Allow-only: a wrong auto-deny wedges a turn, where a wrong auto-allow is
still bounded by whatever the agent's own sandbox permits.

### Queueing the next instruction

<kbd>f</kbd> composes a follow-up for the selected agent. It is delivered when the current turn
ends, so the agent picks it up and keeps going instead of stopping:

```
▶ 1 ⠙ claude  working   1m4s  api        Add retry to webhook client
      └ follow-up: now run the tests · 3 turns · pane:5
```

Unlike a reply this needs no prompt to be open - a working agent is exactly what it is for - and
it reaches agents in other sessions. `ZJ_AGENT_FOLLOWUP=0` opts out.

### Desktop notifications

On by default. When an agent starts `waiting` or `failed`, you get a system notification even
when Zellij is not the focused window - which is exactly when a blocked agent is invisible.

A burst becomes one banner rather than five, each agent is rate-limited to one notification a
minute, and nothing fires while the panel is already on screen. To change what it notifies about:

```kdl
LaunchOrFocusPlugin "file:~/.config/zellij/plugins/zj-agent-mob.wasm" {
    floating true
    notify "waiting,failed,done"   // add done; "" turns notifications off
    notify_sound "true"
}
```

### Fleet status in your status bar

Set `summary_file` and the panel publishes the fleet's state on every change, so
you know whether anyone needs you without opening anything:

```kdl
    summary_file "/tmp/zj-agent-mob.summary"
```

Two files, both written atomically: the prose line (`2 waiting · 3 working`,
empty when nothing needs you) and `<path>.kv` for consumers that would rather
not parse prose:

```
failed=0 waiting=2 working=1 done=0 found=0 total=3
```

The prose line is also piped as `zj-agent-mob-summary`, so
[zjstatus](https://github.com/dj95/zjstatus) can pick it up with its `pipe`
widget. Both formats are a stated contract - see
[the fleet summary](docs/setup.md#the-fleet-summary-in-your-status-bar) for
worked starship, tmux and shell examples.

## Known limitations

- Agents started before `init.sh` ran aren't tracked (no hooks installed yet). Restart them.
- Agents in other sessions report through a status file in `$TMPDIR`. The states that need you (`waiting`, `failed`, `done`) pipe across immediately; quieter ones wait for the next scan (5s).
- Claude has no "permission granted" event, so `waiting` to `working` relies on the next tool-event heartbeat. With `ZJ_AGENT_HEARTBEAT=0`, `waiting` persists until the turn ends.
- Notifications need a notifier on `PATH` (`terminal-notifier` / `osascript` on macOS, `notify-send` on Linux). Without one, everything else still works.

Hitting something not listed here? See [docs/troubleshooting.md](docs/troubleshooting.md).

## Releases

Prebuilt `zj-agent-mob.wasm` binaries and changelogs are on the [releases page](https://github.com/mohseenrm/zj-agent-mob/releases). Pushing a `v*` tag builds the wasm, verifies it exports what Zellij needs, and publishes it automatically.

## License

[Apache-2.0](LICENSE)
