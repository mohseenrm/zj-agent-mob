# Setup

- [Install](#install)
  - [From a release](#from-a-release)
  - [From source](#from-source)
- [The install screen](#the-install-screen)
- [Register the plugin with Zellij](#register-the-plugin-with-zellij)
- [Configuration](#configuration)

## Install

Two options: let `init.sh` download a release for you, or build the wasm yourself. `jq` is required either way.

### From a release

Each release ships three assets: `init.sh`, `zj-agent-mob-hook.sh`, and `zj-agent-mob.wasm`. The installer fetches the two it needs from the same tag it was downloaded from, so one command is the whole install:

```sh
curl -fsSL https://github.com/mohseenrm/zj-agent-mob/releases/download/v0.2.0/init.sh | sh
```

No clone, no Rust toolchain, no manual `target/` directory. To inspect the script first:

```sh
curl -fsSL -O https://github.com/mohseenrm/zj-agent-mob/releases/download/v0.2.0/init.sh
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

## The install screen

After the first run you can do all of this from inside the panel instead: press <kbd>i</kbd> for the install screen.

![The install screen with Claude Code hooks and the plugin installed, Codex hooks absent](img/05-install-partial.png)

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

### Hook script environment

Set these in the environment the *agent* runs in, not the panel's.

| Variable | Default | Meaning |
|---|---|---|
| `ZJ_AGENT_TOOL` | `claude` | Which transcript reader to use (`claude` / `codex`) |
| `ZJ_AGENT_HEARTBEAT` | `1` | Set `0` to skip `PreToolUse`/`PostToolUse` (halves hook volume) |
| `ZJ_AGENT_APPROVE` | `0` | Set `1` to park permission prompts in the panel for <kbd>a</kbd> / <kbd>r</kbd> |
| `ZJ_AGENT_APPROVE_TIMEOUT` | `30` | Seconds a parked prompt waits before falling through to the agent's own prompt |
| `ZJ_AGENT_SPOOL` | `1` | Set `0` to stop writing the cross-session status file. Agents in other sessions then show `found` instead of live status |
| `ZJ_AGENT_SPOOL_DIR` | `$TMPDIR/zj-agent-mob-<uid>/status` | Where status files are written. Created `0700`, since records contain task summaries |
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
