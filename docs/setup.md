# Setup

- [Install](#install)
  - [From a release](#from-a-release)
  - [From source](#from-source)
  - [Reinstalling from a checkout](#reinstalling-from-a-checkout)
- [The install screen](#the-install-screen)
- [Register the plugin with Zellij](#register-the-plugin-with-zellij)
- [Configuration](#configuration)
- [The fleet summary in your status bar](#the-fleet-summary-in-your-status-bar)
  - [The format contract](#the-format-contract)
  - [Worked examples](#worked-examples)

## Install

Two options: let `init.sh` download a release for you, or build the wasm yourself. `jq` is required either way.

### From a release

Each release ships three assets: `init.sh`, `zj-agent-mob-hook.sh`, and `zj-agent-mob.wasm`. The installer fetches the two it needs from the same tag it was downloaded from, so one command is the whole install:

```sh
curl -fsSL https://github.com/mohseenrm/zj-agent-mob/releases/download/v0.9.0/init.sh | sh
```

No clone, no Rust toolchain, no manual `target/` directory. To inspect the script first:

```sh
curl -fsSL -O https://github.com/mohseenrm/zj-agent-mob/releases/download/v0.9.0/init.sh
less init.sh && sh init.sh
```

The URL names an explicit tag rather than `latest` on purpose. The installer, hook script, and wasm are versioned together, so a moving `latest` could pair a new plugin with an older hook on disk, and Zellij caches remote plugins by URL, which would keep serving a stale binary. Grab the newest tag from the [releases page](https://github.com/mohseenrm/zj-agent-mob/releases) and substitute it.

Pin a different release with `--version`:

```sh
sh init.sh --version v0.2.0
```

Naming a version always downloads that release, even if a local build is present, so you get what you asked for.

### From source

```sh
rustup target add wasm32-wasip1
cargo build --release --target wasm32-wasip1
./init.sh
```

From a clone with a local build, `init.sh` downloads nothing.

`init.sh` installs the hook script, copies the plugin, and merges hook entries into `~/.claude/settings.json` and `~/.codex/hooks.json` without disturbing hooks you already have. It is idempotent, so re-running it is safe.

```sh
./init.sh                  # install everything
./init.sh install claude   # just Claude Code's hooks
./init.sh install codex    # just Codex's hooks
./init.sh install plugin   # just copy the built wasm
./init.sh status           # what is installed right now
./init.sh --dry-run        # preview, write nothing (never downloads)
./init.sh uninstall        # remove exactly what was installed
./init.sh uninstall codex  # remove one target only
./init.sh --from-release   # prefer the released wasm over a local build
./init.sh --version v0.2.0 # pin a release; implies --from-release
./init.sh --no-download    # fail rather than fetch anything (offline)
```

By default the installer downloads only what the source tree does not already provide: from a clone with a built wasm it stays entirely local, and from a bare `init.sh` it fetches the hook and plugin. `--from-release` and `--no-download` force each end of that.

> [!IMPORTANT]
> Restart any running `claude` / `codex` sessions after installing. Hooks are read at session start, so existing sessions won't report status.

### Reinstalling from a checkout

`init.sh` always overwrites the hook and the wasm, but two things survive it and
will keep you on old behaviour:

- **Zellij caches compiled plugins.** A new wasm on disk is not the wasm a
  running session has loaded.
- **Spool records outlive a hook change.** A record written by an older hook
  stays in `$TMPDIR` until it is swept.

`scripts/reinstall-local.sh` does the whole cycle, which is what you want when
you are moving the same checkout across machines:

```sh
./scripts/reinstall-local.sh          # build, install, clear both caches
./scripts/reinstall-local.sh --check  # is the install current? exits 1 if not
```

`--check` compares the installed wasm and hook against the checkout and prints
`ok` or `STALE` for each, so you can tell at a glance whether a machine is
actually running what you think it is.

Afterwards, restart your agents *and* start a new Zellij session - a new session
is the reliable way to make Zellij load the new plugin.

## The install screen

After the first run you can do all of this from inside the panel instead: press <kbd>i</kbd> for the install screen.

![The install screen with Claude Code hooks and the plugin installed, Codex hooks absent](img/05-install-partial.png)

With hooks installed but no agent running yet, the panel says so rather than
looking broken - start `claude` or `codex` in any pane and it fills in:

![The empty state telling you to start claude or codex in a pane](img/00-empty.png)

Each row toggles: pressing its key installs when absent and uninstalls when present.

If neither agent is hooked, the panel skips the empty state and offers the same install directly:

![The setup screen listing four quick actions: install for Claude Code, for Codex, for both, or quit](img/01-setup.png)

The screen shells out to the copy of the installer that `init.sh` leaves at `~/.config/zj-agent-mob/install.sh`, so it works regardless of where you cloned the repo. This needs Zellij's "Run commands" permission, which the plugin requests on first load.

## Register the plugin with Zellij

`init.sh` copies the plugin to `~/.config/zellij/plugins/zj-agent-mob.wasm`, but Zellij still needs to know how to open it.

### As a keybinding

Add to `~/.config/zellij/config.kdl`:

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

### In a layout

To have the panel present from the start, add it to `~/.config/zellij/layouts/default.kdl`:

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

## Configuration

Plugin config goes in the same block as the launch action:

```kdl
LaunchOrFocusPlugin "file:~/.config/zellij/plugins/zj-agent-mob.wasm" {
    floating true
    move_to_focused_tab true
    popup_on_waiting true
}
```

| Key | Default | Meaning |
|---|---|---|
| `popup_on_waiting` | `true` | Auto-show the panel when an agent needs input. Set `false` to only ever open it yourself |
| `discover` | `true` | Scan process environments for agents that have not fired a hook yet, including ones in other sessions. Set `false` to show only agents that have reported |
| `notify` | `waiting,failed` | Which transitions raise a desktop notification. Any of `waiting`, `idlewait`, `failed`, `done`, comma-separated. `""` disables them |
| `notify_cooldown` | `60` | Seconds before the same agent may notify again, so a flapping row cannot spam you |
| `notify_sound` | `false` | Play a sound with the notification |
| `summary_file` | unset | Write the fleet summary here on every change, for a status bar to render. Also writes `<path>.kv` for parsing. Unset means nothing is published. See [the fleet summary](#the-fleet-summary-in-your-status-bar) |

## The fleet summary in your status bar

Set `summary_file` and the panel publishes the fleet's state on every change,
for anything outside Zellij to render. This is the difference between a panel
you open and a number that is always in front of you.

Configuration goes wherever you already declare the plugin - in a keybinding:

```kdl
keybinds {
    shared_except "locked" {
        bind "Ctrl a" {
            LaunchOrFocusPlugin "file:~/.config/zellij/plugins/zj-agent-mob.wasm" {
                floating true
                summary_file "/tmp/zj-agent-mob.summary"
            }
        }
    }
}
```

or in a layout:

```kdl
floating_panes {
    pane {
        plugin location="file:~/.config/zellij/plugins/zj-agent-mob.wasm" {
            summary_file "/tmp/zj-agent-mob.summary"
        }
    }
}
```

Two files are written, both replaced atomically via `mv`, so a consumer polling
them never reads a half-written count:

| File | Contents | For |
|---|---|---|
| `$summary_file` | `2 waiting · 1 working` | Rendering directly |
| `$summary_file.kv` | `failed=0 waiting=2 working=1 done=0 found=0 total=3` | Parsing |

A `zellij pipe --name zj-agent-mob-summary` also carries the prose line, for
consumers that are themselves Zellij plugins.

### The format contract

Both lines are stable, and worth relying on:

- **Prose** (`$summary_file`) lists only non-zero counts, in urgency order
  (`failed`, `waiting`, `working`, `done`), joined by ` · `. It is **empty when
  nothing needs you**, which is what keeps a status bar quiet at rest.
- **Machine-readable** (`$summary_file.kv`) always carries **every** key, zeros
  included, so `waiting=0` is never ambiguous with a key that was left out.
  `waiting` folds in `idle-wait`; `working` folds in `compact`; `found` is
  agents seen by the scan that have never reported; `total` is every row.

Neither file exists until the first publish, so read defensively.

### Worked examples

**Starship** - a module that stays invisible when nothing needs you:

```toml
# ~/.config/starship.toml
[custom.agents]
command = "cat /tmp/zj-agent-mob.summary 2>/dev/null"
when = "test -s /tmp/zj-agent-mob.summary"
format = "[$output]($style) "
style = "bold yellow"
shell = ["sh", "-c"]
```

**tmux** - the prose line, refreshed on tmux's own interval:

```sh
set -g status-right '#(cat /tmp/zj-agent-mob.summary 2>/dev/null) | %H:%M'
```

**Anything that needs a decision**, reading the `k=v` line rather than parsing
prose. One field, no sourcing:

```sh
waiting=$(awk -v RS=' ' -F= '$1=="waiting"{print $2}' \
  /tmp/zj-agent-mob.summary.kv 2>/dev/null)

[ "${waiting:-0}" -gt 0 ] && printf 'agents blocked: %s\n' "$waiting"
```

**Zellij's own status bar** cannot shell out, so it reads the pipe rather than
the file - see [how it works](how-it-works.md) for the pipe's shape.

### Hook script environment

Set these in the environment the *agent* runs in, not the panel's.

| Variable | Default | Meaning |
|---|---|---|
| `ZJ_AGENT_TOOL` | `claude` | Which transcript reader to use (`claude` / `codex`) |
| `ZJ_AGENT_HEARTBEAT` | `1` | Set `0` to skip `PreToolUse`/`PostToolUse` (halves hook volume) |
| `ZJ_AGENT_APPROVE` | `1` | Set `0` to stop parking permission prompts in the panel for <kbd>a</kbd> / <kbd>r</kbd> |
| `ZJ_AGENT_APPROVE_TIMEOUT` | `30` | Seconds a parked prompt waits before falling through to the agent's own prompt |
| `ZJ_AGENT_APPROVE_RULES` | `~/.config/zj-agent-mob/approve.rules` | Rules that answer a prompt without asking. One `allow <tool> [arg-prefix]` per line; <kbd>A</kbd> appends one |
| `ZJ_AGENT_FOLLOWUP` | `1` | Set `0` to stop delivering a follow-up queued with <kbd>f</kbd> when the turn ends |
| `ZJ_AGENT_CONTEXT` | `1` | Set `0` to stop telling an agent about other agents working in the same directory |
| `ZJ_AGENT_SLOW_TOOL` | `10` | Seconds a tool call must take before its duration is shown on the detail line |
| `ZJ_AGENT_SPOOL` | `1` | Set `0` to stop writing the cross-session status file. Agents in other sessions then show `found` instead of live status |
| `ZJ_AGENT_SPOOL_DIR` | `$TMPDIR/zj-agent-mob-<uid>/status` | Where status files are written. Created `0700`, since records contain task summaries |
| `ZJ_AGENT_FANOUT` | `1` | Set `0` to stop piping `waiting` / `failed` / `done` straight to panels in other sessions. They then wait for the next poll instead |
| `ZJ_AGENT_PLUGIN` | `file:~/.config/zellij/plugins/zj-agent-mob.wasm` | Plugin path |
| `ZJ_AGENT_DEBUG` | `0` | Set `1` to log events to `~/.cache/zj-agent-mob/hook.log` |

### Installer environment

Mostly useful for testing against throwaway directories:

| Variable | Default |
|---|---|
| `ZJ_AGENT_HOOK_DIR` | `~/.config/zj-agent-mob` |
| `ZJ_AGENT_PLUGIN_DIR` | `~/.config/zellij/plugins` |
| `CLAUDE_CONFIG_DIR` | `~/.claude` |
| `CODEX_HOME` | `~/.codex` |
