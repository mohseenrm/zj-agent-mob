# Local development

- [Getting started](#getting-started)
- [Iterating against a live session](#iterating-against-a-live-session)
- [Cutting a release](#cutting-a-release)
- [Why a bin target](#why-a-bin-target)
- [Module layout](#module-layout)
- [Rendering note](#rendering-note)

## Getting started

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
shellcheck --shell=sh init.sh scripts/zj-agent-mob-hook.sh tests/e2e-hook.sh
./tests/e2e-hook.sh
```

### Test layers

| Layer | Where | Covers |
|---|---|---|
| Unit | `src/*.rs`, beside the code | The state machine and layout, starting from an already-parsed pipe message |
| End-to-end | [`tests/e2e-hook.sh`](../tests/e2e-hook.sh) | The hook script: hook-event JSON in, `zellij pipe --args` out |
| Installer | CI `shell` job | Idempotent re-install, per-target selection, parseable `status` output |

`tests/e2e-hook.sh` covers the seam the Rust tests cannot reach: event-to-status mapping, the cases that must stay silent (no `$ZELLIJ_PANE_ID`, `ZJ_AGENT_HEARTBEAT=0`, unknown or malformed events), Claude `ai-title` and Codex rollout summaries, sanitizing, shell-injection through the task and `cwd`, and the always-exit-0 contract.

It stubs `zellij` with a script that records its argv, so it needs no Zellij, no agent and no pane, and runs in about a second:

```sh
./tests/e2e-hook.sh
```

## Iterating against a live session

```sh
cargo build --release --target wasm32-wasip1
./init.sh install plugin

# Zellij caches compiled plugins, so force a reload or the old build stays live.
zellij action launch-or-focus-plugin --skip-plugin-cache --floating \
  "file:$HOME/.config/zellij/plugins/zj-agent-mob.wasm"
```

Feed the panel a status without running a real agent:

```sh
zellij pipe --name agent-status \
  --plugin "file:$HOME/.config/zellij/plugins/zj-agent-mob.wasm" \
  --args "pane_id=$ZELLIJ_PANE_ID,tool=claude,status=waiting,task=manual test"
```

Test the installer without touching your real config by pointing it at throwaway directories:

```sh
export ZJ_AGENT_HOOK_DIR=/tmp/zj/hooks ZJ_AGENT_PLUGIN_DIR=/tmp/zj/plugins
export CLAUDE_CONFIG_DIR=/tmp/zj/claude CODEX_HOME=/tmp/zj/codex
./init.sh install && ./init.sh status && ./init.sh uninstall
```

## Cutting a release

Releases are published by [`.github/workflows/release.yml`](../.github/workflows/release.yml) when a `v*` tag is pushed. The workflow builds the wasm, asserts it exports the six symbols Zellij needs, checks the tag matches `Cargo.toml`, then creates the GitHub release with generated notes and the wasm attached.

The tag and `Cargo.toml` version must agree or the workflow fails on purpose, so bump the version first:

```sh
# 1. bump `version` in Cargo.toml, then refresh Cargo.lock
cargo build --release --target wasm32-wasip1
git commit -am "chore: release v0.2.0"

# 2. tag and push; the workflow does the rest
git tag -a v0.2.0 -m "v0.2.0"
git push origin main --follow-tags
```

Published releases are listed on the [releases page](https://github.com/mohseenrm/zj-agent-mob/releases).

## Why a bin target

The crate builds a **bin** target (`src/main.rs`), not just a cdylib. Zellij's loader needs the WASI `_start` export, which only a bin provides; a bare cdylib fails at load with `could not find exported function`. `register_plugin!` also generates its own `fn main()`, so it must be invoked in `main.rs`. The lib target (`src/lib.rs`) holds all the logic purely so `cargo test` can run it natively.

To check a build has the right exports:

```sh
wasm-objdump -x target/wasm32-wasip1/release/zj-agent-mob.wasm | grep -A8 'Export\['
```

You want `_start`, `load`, `update`, `render`, `pipe`, and `plugin_version`. CI asserts all six.

## Module layout

| File | Lines | Tests | Role |
|---|---|---|---|
| `main.rs` | 6 | | `register_plugin!` + WASI entry point |
| `lib.rs` | 33 | | Module wiring and shared constants |
| `plugin.rs` | 260 | | Zellij lifecycle: permissions, subscriptions, `render` |
| `state.rs` | 194 | 18 | State machine: pipe handling, pane reconciliation |
| `install.rs` | 378 | 22 | Install screen: state, toggles, installer output parsing |
| `keys.rs` | 191 | 12 | Keyboard: selection, jump-to-pane, two-step kill |
| `agent.rs` | 111 | 10 | One agent, and how its row is built |
| `ribbon.rs` | 65 | 7 | Ribbon line serialization |
| `status.rs` | 55 | | The four states and their presentation |
| `util.rs` | 38 | 2 | `fmt_elapsed`, `truncate` |
| `host.rs` | 31 | | Host-call shim |
| `style.rs` | 11 | | ANSI constants |

Line counts exclude tests. Tests live beside the code they cover, 71 in total, none needing a running Zellij, plus the 41 end-to-end cases in `tests/e2e-hook.sh`.

Zellij host calls (`focus_terminal_pane`, `hide_self`, `run_command`, ...) are WASM imports with no native symbol, so they're behind the `host` shim that no-ops off-wasm. That keeps the whole state machine and all layout code unit-testable with a plain `cargo test`.

## Rendering note

The panel uses plain ANSI, not Zellij's `Text` / ribbon UI components. Those serialize to a DCS sequence that repositions the cursor itself, so consecutive components collapse onto one grid row. The cost is that colours are fixed 256-colour codes rather than following your Zellij theme.

If you do use the component API elsewhere, note that `Text::color_range()` indices are **byte** offsets (`serialize()` encodes via `as_bytes()`), not character offsets. Character counts corrupt the payload for multi-byte glyphs like `▶` or the braille spinner.
