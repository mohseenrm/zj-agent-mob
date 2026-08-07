# Distribution spec: wasm + config, no external deps

Status: proposal. Nothing here is implemented yet.

The goal: a user installs zj-agent-mob by adding a block to `~/.config/zellij/config.kdl`.
No clone, no `curl`, no `jq`, no `init.sh`, no Rust toolchain. One wasm and one config file.

## Where we are today

Installing requires, in order: clone the repo (or download a release asset through
`gh`, because the repo is private), have `jq` on `PATH`, run `./init.sh`, which copies
the wasm into `~/.config/zellij/plugins/`, copies `scripts/zj-agent-mob-hook.sh` to
`~/.config/zj-agent-mob/hook.sh`, self-copies to `~/.config/zj-agent-mob/install.sh`,
and merges hook entries into `~/.claude/settings.json` and `~/.codex/hooks.json`. Then
the user hand-edits `config.kdl` for a keybinding.

Five artifacts land on disk outside the wasm: `hook.sh`, `install.sh`, and edits to two
agent settings files plus `config.kdl`. Four external dependencies: `git`/`gh`, `jq`,
`sh`, and Zellij's `RunCommands` permission.

## The four dependencies, and what each costs to remove

### 1. The download (`git` / `gh` / `curl`)

**Removable today, no code change.** Zellij 0.44 already resolves `https:` plugin URLs:
`zellij-utils-0.44.3/src/input/layout.rs:629` maps the `https`/`http` scheme to
`RunPluginLocation::Remote`, and Zellij downloads and caches the wasm itself. So

```kdl
LaunchOrFocusPlugin "https://github.com/mohseenrm/zj-agent-mob/releases/latest/download/zj-agent-mob.wasm"
```

replaces the entire download step. Two blockers, both non-technical:

- **The repo is private.** Release assets 404 for unauthenticated clients, and Zellij's
  fetcher sends no credentials. Making the repo public is a prerequisite for this whole
  spec, not an optional extra.
- `latest/download/` is a moving target. Pin a version in the documented snippet
  (`download/v0.1.0/`) and let users opt into `latest`.

### 2. `jq` in the hook

This is the hard one, and it is what actually blocks "no external deps".

`scripts/zj-agent-mob-hook.sh` uses `jq` three times: to parse the hook event on stdin,
to pull `aiTitle`/`lastPrompt` out of a Claude transcript, and to scan a Codex rollout.
`init.sh` uses it to merge settings JSON.

The fix is to move parsing out of the shell and into the wasm, which is already a JSON
consumer's natural home. The hook currently does the work because `zellij pipe --args`
takes `key=value` pairs and the plugin's `handle_status` (`src/state.rs:63`) reads
pre-split fields. But `zellij pipe` can carry an arbitrary payload on stdin, and
`PipeMessage` exposes it. So:

- The hook becomes: check `$ZELLIJ_PANE_ID`, `cat` stdin, and forward it verbatim via
  `zellij pipe --name agent-event`. No parsing at all. That is pure POSIX `sh` plus the
  `zellij` binary the user already has.
- The plugin grows a small JSON reader that takes the raw hook event and derives
  `event`, `session_id`, `cwd`, `transcript_path`, `tool_name` itself, then maps event
  to status with the same table the shell uses now.

Cost: a JSON parser in the wasm. `serde_json` is the obvious choice; it costs roughly
100-200KB before `opt-level="z"` + LTO, which the release profile already sets. Given
the plugin is a local UI and not a hot path, that is acceptable. A hand-rolled reader
would be smaller but is not worth the bug surface for arbitrary transcript content.

**The transcript read is the subtlety.** Today the shell does `tail -n 300` on a
multi-MB transcript. A wasm plugin cannot read arbitrary host paths: Zellij only mounts
the plugin's own data dir and the cwd. Three options, in preference order:

1. **Keep the `tail` in the hook, drop only the `jq`.** The hook sends the last N
   transcript lines as part of the piped payload and the plugin parses them. `tail` is
   POSIX and present everywhere `sh` is. This keeps zero non-POSIX deps and needs no new
   permission. Recommended.
2. Have the plugin `run_command` a `tail`, which reintroduces `RunCommands` for the
   common path rather than just the install screen. Worse.
3. Parse the transcript in wasm via a host filesystem mount. Not available for arbitrary
   `$HOME` paths. Rejected.

Option 1 means the hook stays a shell script but its only tools are `cat`, `tail`, and
`zellij`. That satisfies "no external deps" in the sense that matters: nothing the user
must install.

### 3. `init.sh` and the settings merge

Once the hook needs no `jq`, the installer is the last `jq` user. Two paths:

**Path A - the plugin writes the hooks itself.** It already has `RunCommands` and an
install screen that shells out. Instead of driving `install.sh`, it would need to
read-modify-write two JSON files, which means the same filesystem problem as the
transcript: no arbitrary host FS access from wasm. It would have to `run_command` a
shell to do the write, i.e. generate the script inline. Doable but ugly, and a
half-written settings file is a genuinely bad failure mode.

**Path B - stop merging entirely.** Claude Code and Codex both support a hooks file that
the user points at. If the documented install is "add this to your settings", the
installer disappears and the user owns the edit. This trades convenience for
transparency and removes ~400 lines of `jq` merge logic and its e2e suite.

Recommendation: **keep `init.sh` as the convenience path, but make it optional and not
required for a working install.** Document the manual settings block so a user who
declines to run a script can still install. `jq` then becomes a dependency of the
convenience path only, which is a defensible place for it. Removing the merge logic
outright would regress the "does not disturb hooks you already have" property, which is
real value.

### 4. The `RunCommands` permission

Once hooks are documented as a manual edit and the wasm no longer drives `install.sh`,
`load()` (`src/plugin.rs:23`) can drop `PermissionType::RunCommands`, leaving only
`ReadApplicationState` and `ChangeApplicationState`. Fewer permission prompts on first
load, and the plugin stops being able to execute arbitrary shell, which is a meaningful
reduction in what a user has to trust.

This is contingent on dropping the in-panel install screen, or on accepting that the
install screen is the one feature that asks for the extra permission. Worth deciding
explicitly rather than by default.

## What "config" should mean

Everything currently passed by environment variable or baked into the hook should move
into the KDL block, which `load()` already receives as a `BTreeMap<String, String>`
(`src/plugin.rs:13`). Today only `popup_on_waiting` is read. Proposed surface:

```kdl
keybinds {
    shared_except "locked" {
        bind "Ctrl a" {
            LaunchOrFocusPlugin "https://github.com/mohseenrm/zj-agent-mob/releases/download/v0.1.0/zj-agent-mob.wasm" {
                floating true
                move_to_focused_tab true
                popup_on_waiting true
                heartbeat true
                transcript_lines 300
            }
        }
    }
}
```

`ZJ_AGENT_HEARTBEAT` and the hardcoded `tail -n 300` become config keys. `ZJ_AGENT_TOOL`
stays an env var because it is per-agent and set by the hook entry itself.

## Proposed end state

The user does two things:

1. Add the KDL block above to `~/.config/zellij/config.kdl`.
2. Add one hook entry per agent, either by running `init.sh` or by pasting a documented
   JSON block.

Step 2 cannot go away: an agent will not report status unless its own settings tell it
to run something. That is a property of Claude Code and Codex, not of this project. The
honest version of "no external deps" is therefore: **no external deps beyond a POSIX
shell and the `zellij` binary**, with `jq` demoted to an optional convenience.

## Work items

Ordered by dependency, not by size.

| # | Item | Blocks | Notes |
|---|---|---|---|
| 1 | Make the repo public | 2 | Prerequisite for remote plugin URLs. Non-technical. |
| 2 | Document the `https:` plugin URL; pin a version | - | No code change; Zellij 0.44 already supports it. |
| 3 | Add a JSON reader to the plugin (`serde_json`) | 4 | Watch the wasm size delta; release profile already optimizes for size. |
| 4 | Move hook-event parsing into the plugin; hook forwards raw stdin | 5, 6 | New pipe name `agent-event`; keep `agent-status` for one release. |
| 5 | Move transcript extraction into the plugin; hook sends `tail -n N` output | - | Keeps `tail`, drops `jq` from the hook. |
| 6 | Add `heartbeat` / `transcript_lines` config keys to `load()` | - | Replaces `ZJ_AGENT_HEARTBEAT`. |
| 7 | Document the manual settings-JSON block for both agents | 8 | Makes `init.sh` optional. |
| 8 | Decide: keep the install screen (and `RunCommands`) or drop both | - | Needs a call. See below. |
| 9 | Update e2e suites: `tests/e2e-hook.sh` asserts the parsed `--args`, which item 4 changes | 4 | The suite will fail loudly; that is correct. |

## Open decisions

Two things genuinely need a call before implementation, because they change the shape of
the work rather than its size:

- **Is the in-panel install screen worth the `RunCommands` permission?** Keeping it means
  the plugin can execute arbitrary shell forever. Dropping it means ~660 lines of
  `src/install.rs` and its e2e suite go away, first-run gets simpler, but users lose
  install-without-leaving-Zellij.
- **Does `init.sh` survive?** Recommendation above is yes-but-optional. The alternative
  is deleting it and owning a docs-only install.

## What this does not fix

- Users still restart running agent sessions after installing hooks.
- The wasm is still built per-release by CI; nothing here changes the build.
- Zellij's remote-plugin cache means a `latest` URL can serve a stale wasm until the
  cache is cleared. Pinning versions sidesteps this.
