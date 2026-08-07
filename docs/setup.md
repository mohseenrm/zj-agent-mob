# Setup

- [Install](#install)
  - [From a release](#from-a-release)
  - [From source](#from-source)
- [The install screen](#the-install-screen)
- [Register the plugin with Zellij](#register-the-plugin-with-zellij)
- [Configuration](#configuration)

## Install

Two options: download the prebuilt `zj-agent-mob.wasm` from a release, or build it yourself. Either way, `init.sh` does the rest, and `jq` is required for both.

### From a release

Grab `zj-agent-mob.wasm` from the [latest release](https://github.com/mohseenrm/zj-agent-mob/releases/latest) ([all releases](https://github.com/mohseenrm/zj-agent-mob/releases)). No Rust toolchain needed.

> [!NOTE]
> This repository is **private**, so release assets need an authenticated download. Plain `curl` gets a 404 until the repo is public. The `gh` commands below work today; the `curl` ones become the simpler path once it is public.

`init.sh` reads the plugin from `target/wasm32-wasip1/release/`, so put the download there and it installs like a local build:

```sh
mkdir -p target/wasm32-wasip1/release
gh release download --repo mohseenrm/zj-agent-mob v0.1.0 \
  --pattern zj-agent-mob.wasm --dir target/wasm32-wasip1/release
./init.sh
```

To skip the repo entirely, put the wasm directly into the plugin directory and install the hooks separately:

```sh
mkdir -p ~/.config/zellij/plugins
gh release download --repo mohseenrm/zj-agent-mob v0.1.0 \
  --pattern zj-agent-mob.wasm --dir ~/.config/zellij/plugins
./init.sh install claude codex   # hooks only; the plugin is already in place
```

Once the repo is public, either download becomes a plain `curl` against the stable `latest` URL:

```sh
curl -fsSL -o target/wasm32-wasip1/release/zj-agent-mob.wasm \
  https://github.com/mohseenrm/zj-agent-mob/releases/latest/download/zj-agent-mob.wasm
```

### From source

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
./init.sh status           # what is installed right now
./init.sh --dry-run        # preview, write nothing
./init.sh uninstall        # remove exactly what was installed
./init.sh uninstall codex  # remove one target only
```

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
| `popup_on_waiting` | `true` | Auto-show the panel when an agent needs input |

### Hook script environment

| Variable | Default | Meaning |
|---|---|---|
| `ZJ_AGENT_TOOL` | `claude` | Which transcript reader to use (`claude` / `codex`) |
| `ZJ_AGENT_HEARTBEAT` | `1` | Set `0` to skip `PreToolUse`/`PostToolUse` (halves hook volume) |
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
