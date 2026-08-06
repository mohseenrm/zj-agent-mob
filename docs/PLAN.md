# zj-agent-mob: Plan

A Zellij plugin that provides herdr-like agent monitoring inside a Zellij session: track Claude Code and Codex agents running in panes, show live status (working / idle / awaiting input), notify when an agent needs attention, and jump to (or kill) any agent with one keypress.

Unlike herdr (which is its own terminal multiplexer with an agent layer built in), this reuses Zellij as the multiplexer and implements only the agent layer as a WASM plugin plus hook scripts.

## Architecture

```
┌─────────────────────────────  zellij session  ─────────────────────────────┐
│                                                                            │
│  pane 1: claude          pane 2: codex           plugin pane (floating)    │
│  ┌────────────────┐      ┌────────────────┐      ┌──────────────────────┐  │
│  │ Claude Code    │      │ Codex CLI      │      │ zj-agent-mob.wasm    │  │
│  │  hooks fire ───┼──┐   │  hooks fire ───┼──┐   │  - agent registry    │  │
│  └────────────────┘  │   └────────────────┘  │   │  - TUI list + status │  │
│                      │                       │   │  - focus/kill panes  │  │
│                      ▼                       ▼   └──────────▲───────────┘  │
│              zj-agent-mob-hook.sh (one script, both tools)  │              │
│                      │                                      │              │
│                      └── zellij pipe --plugin file:...wasm ─┘              │
│                          --args "pane_id=$ZELLIJ_PANE_ID,..."              │
└────────────────────────────────────────────────────────────────────────────┘
```

Three deliverables:

1. **Plugin** (`zj-agent-mob.wasm`) - Rust, `zellij-tile`, compiled to `wasm32-wasip1`.
2. **Hook script** (`zj-agent-mob-hook.sh`) - a single POSIX shell script invoked by both Claude Code and Codex hooks. Reads the hook JSON from stdin, maps `hook_event_name` to a status, and forwards it to the plugin via `zellij pipe`.
3. **Init script** (`init.sh`) - one-time installer that merges hook config into `~/.claude/settings.json` and writes `~/.codex/hooks.json`.

### Why this works

- Hook processes spawned by an agent inherit the pane's environment, so `$ZELLIJ_PANE_ID` (set by Zellij in every terminal pane) identifies the exact pane. `$ZELLIJ_SESSION_NAME` scopes events to the current session.
- `zellij pipe --plugin file:...wasm` auto-launches the plugin if it isn't running yet and delivers the message to `pipe()` - no daemon, no socket, no state files needed.
- Agents outside any Zellij session have no `$ZELLIJ_PANE_ID`; the hook script exits immediately in that case. This gives "only monitor agents in the current session" for free (each session's plugin instance only receives pipes from panes in that session).

## Status model

| Status | Meaning | Set by |
|---|---|---|
| `working` | Agent is processing a turn | `UserPromptSubmit` (both tools); refreshed by `PreToolUse`/`PostToolUse` heartbeats (optional, phase 2) |
| `waiting` | Agent needs user input NOW (permission prompt / question) | Claude: `Notification` (matcher `permission_prompt\|idle_prompt`), `PermissionRequest`. Codex: `PermissionRequest` |
| `done` | Turn finished, agent idle with unseen result | `Stop` (both tools), Codex `notify` `agent-turn-complete` |
| `idle` | Agent session open, nothing new | `SessionStart`; `done` decays to `idle` once the pane is visited |
| (removed) | Session ended or pane closed | `SessionEnd`, or `PaneClosed`/`CommandPaneExited` from Zellij |

`waiting` and `done` are the notify-worthy states. `done` vs `idle` distinction is borrowed from herdr: "finished while you were elsewhere" is the thing you actually want surfaced.

## Component design

### 1. Plugin (Rust)

State:

```rust
struct AgentEntry {
    pane_id: u32,            // terminal pane id, from $ZELLIJ_PANE_ID
    tool: Tool,              // Claude | Codex
    session_id: String,      // from hook payload
    status: Status,          // Working | Waiting | Done | Idle
    cwd: String,             // from hook payload; basename shown as project label
    task: Option<String>,    // ai-title (Claude) / first user_message (Codex); see below
    label: Option<String>,   // user-set override via `agent-label` pipe (phase 2)
    detail: Option<String>,  // last tool activity, or pending permission subject
    turns: u32,              // count of UserPromptSubmit events
    status_since: f64,       // timestamp of last status change -> elapsed display
    last_event_at: f64,      // for staleness display
    title: String,           // pane title from PaneManifest (final fallback for task)
}
struct State {
    agents: Vec<AgentEntry>,     // display order
    selected: usize,
    panes: PaneManifest,         // latest snapshot
    throbber_frame: usize,
}
```

Lifecycle:

- `load()`: `request_permission(&[ReadApplicationState, ChangeApplicationState])`, `subscribe(&[PaneUpdate, TabUpdate, Key, Timer, PermissionRequestResult])`, `set_selectable(true)`, arm `set_timeout(0.25)` for the throbber (only re-arm while any agent is `working`).
- `pipe(msg)`: handle `PipeMessage { name: "agent-status", args: { pane_id, tool, status, session_id, cwd, task, detail } }`. Upsert the `AgentEntry`, treating empty `task`/`detail` as "leave unchanged" (see hook script notes); bump `turns` on `working` transitions from `UserPromptSubmit`; set `status_since` only when `status` actually changes, so the elapsed timer doesn't reset on every heartbeat. Return `true` to re-render. Also handle `name: "agent-label"` for the manual override.
- `update(Event::PaneUpdate)`: reconcile - drop entries whose pane is gone (handles kill/crash without a `SessionEnd` hook firing), pull pane titles, detect which tab each agent pane is on.
- `update(Event::Timer)`: advance throbber frame, re-arm while anything is `working`.
- `update(Event::Key)`: keybindings below.

Keybindings (while plugin pane is focused):

| Key | Action |
|---|---|
| `j`/`k`, `↓`/`↑` | Move selection |
| `Enter` | `focus_terminal_pane(pane_id, true, false)` then `hide_self()` - jumps to the exact pane, across tabs, unhiding floating/suppressed panes |
| `1`-`9` | Jump directly to agent N (the one-keypress path from a notification) |
| `x` | Kill: `send_sigint_to_pane_id(PaneId::Terminal(id))`; pressed twice (or `X`) → `close_terminal_pane(id)` (kills process + removes pane) |
| `d` | Mark `done` → `idle` (dismiss) |
| `Esc`/`q` | `hide_self()` |

Notification UX ("notify user when agent needs input"):

- Plugin pane title updates to a summary, e.g. `agents: 1 waiting, 2 working` (visible in the tab bar even when hidden - cheap ambient signal).
- When a status transitions to `waiting`/`done` and the plugin is hidden: `show_self(true)` as a floating pane (configurable: `popup_on_waiting=true`). Selection pre-set to that agent, so the flow is: toast appears → press `Enter` → you're in the agent's pane. One keypress.
- Optional (config flag): terminal bell / desktop notification via OSC 9 escape in `render()`.

Rendering:

- `render(rows, cols)` prints to stdout; each call starts from a cleared pane, so redraw the whole frame every time (makes the throbber trivial: frame counter + `set_timeout`).
- Use Zellij's built-in UI components (`zellij_tile::ui_components`) so the plugin follows the user's theme automatically:
  - `Table::new().add_styled_row(vec![Text, ...])` + `print_table_with_coordinates` for the agent list columns (n | throbber/icon | tool | status | cwd | tab).
  - `Text::new(s).color_range(idx, range)` for emphasis - indices 0-3 map to theme colors (no arbitrary RGB in the component protocol; raw ANSI SGR is the escape hatch if exact colors are ever needed).
  - `Text::new(s).selected()` on the row under the cursor - Zellij paints the theme's selection background, no manual highlighting.
  - `print_ribbon` for the bottom keybinding hints bar.

Layout sketch:

```
 zj-agent-mob                          1 waiting · 1 working · 1 done
 ────────────────────────────────────────────────────────────────────
 ▶ 1 ⠋ claude  working  2m14s  api      Add retry to webhook client
       └ Edit src/webhook.rs · 47 turns · tab:2 · pane:3

   2 ● codex   waiting  8s     web      Fix flaky checkout test
       └ needs approval: rm -rf node_modules · tab:1 · pane:5

   3 ✓ claude  done     5m01s  dotfiles Review zellij plugin docs
       └ finished · 12 turns · tab:1 · pane:2
 ────────────────────────────────────────────────────────────────────
 ↵ jump  1-9 quick-jump  x kill  d dismiss  q hide
```

Per-agent fields, and where each comes from:

| Field | Source |
|---|---|
| index, throbber/icon | plugin-local |
| tool | hook config (`ZJ_AGENT_TOOL`) |
| status | hook event mapping |
| elapsed (`2m14s`) | time since last status transition, plugin-local |
| project (`api`) | basename of `cwd` from hook payload |
| **task summary** | transcript-derived, see below |
| **activity detail** | `tool_name` + first arg from `PreToolUse`/`PostToolUse`; for `waiting`, the pending permission subject |
| turn count | count of `UserPromptSubmit` events seen for that session |
| tab / pane | `PaneManifest` reconciliation |

Compact mode (`layout=compact` config, or narrow panes): drop the `└` detail line and render one row per agent as a `Table`. The plugin gets `cols` in `render()`, so it can degrade automatically: hide detail lines under ~70 cols, drop `cwd` under ~50.

### Task summary: what the agent is actually working on

Verified against real transcripts on this machine; no goal state is persisted anywhere, so this is derived from the session transcript. The hook payload gives `transcript_path` (Claude) / `session_id` (both), so the hook script can extract a summary and pass it in the pipe args.

**Claude Code** - the transcript JSONL contains an `ai-title` record type, which is Claude's own generated title for the session. Confirmed live:

```console
$ jq -c 'select(.type=="ai-title")' "$transcript_path" | tail -1
{"type":"ai-title","aiTitle":"Review Zellij plugin UI rendering documentation","sessionId":"c101a82b-..."}
```

This is exactly the "what task were they working on" field, already summarized and updated as the session evolves. Extraction (last record wins):

```sh
task=$(jq -r 'select(.type=="ai-title") | .aiTitle' "$transcript_path" 2>/dev/null | tail -1)
```

Fallback chain, in order: `ai-title` → `last-prompt` (`.lastPrompt`, also present in the transcript) → first user message → pane title.

**Codex** - no `ai-title` equivalent. Its rollout files live at `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<session_id>.jsonl`. Raw `response_item` records are polluted with `AGENTS.md` / `<INSTRUCTIONS>` preamble, but `event_msg` records of subtype `user_message` are the clean prompts the TUI shows. Confirmed:

```sh
task=$(jq -r 'select(.type=="event_msg") | .payload | select(.type=="user_message") | .message' "$rollout" \
  | head -1 | tr '\n' ' ' | cut -c1-60)
```

Codex `session_meta.payload` also carries `cwd`, `git`, and `model` if richer labels are wanted later.

Because `transcript_path` may be null for Codex, resolve the rollout by globbing for `*-$session_id.jsonl` under `~/.codex/sessions`.

**Goals**: Claude Code's `/goal` (session-scoped Stop hook) leaves no on-disk artifact - I checked this session's transcript and `~/.claude/` and found no goal record. So "goal" is not directly readable. Two options: (a) treat `ai-title` as the task label (recommended for v1, it tracks the real work anyway), or (b) phase 2, let users set an explicit label themselves via `zellij pipe --name agent-label --args "pane_id=...,label=..."`, which also gives a manual override for any tool.

Cost note: reading a transcript on every hook event is wasteful (they reach tens of MB - this session's was already 483KB). Mitigations: only re-extract the summary on `SessionStart`, `UserPromptSubmit`, and `Stop` (not on tool events); and read `ai-title` from a bounded `tail -n 300` window rather than scanning the whole file, since the record repeats every few turns (measured: 9 occurrences, last one ~5KB from EOF).

Use `tail -n` (lines), not `tail -c` (bytes). A byte-bounded cut lands mid-line and `jq` aborts the entire stream on the partial first record - verified: `jq: parse error: Invalid numeric literal at line 1, column 9`, yielding an empty summary. If a byte bound is preferred for very long lines, pipe through `sed '1d'` to discard the partial record.

Styling per status: `working` = throbber (`⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏`) + default color; `waiting` = `●` bold red/orange (strongest visual weight); `done` = `✓` green; `idle` = `○` dim. Task summary rendered in a brighter emphasis color than the dim `└` metadata line, so the eye lands on status icon → task → detail. With `Text::color_range()` limited to 4 theme indices, the budget is: 0 = status, 1 = task summary, 2 = tool/project, 3 = dim metadata.

### 2. Hook script (`zj-agent-mob-hook.sh`)

One script for both tools; both send the same envelope shape (`hook_event_name`, `session_id`, `cwd`) as JSON on stdin.

**Does this need to be a shell script?** No, and there's a real tradeoff. The hook runs on *every* event of every agent, so startup cost is the deciding factor.

| Option | Startup | Deps | Verdict |
|---|---|---|---|
| `sh` + `jq` | ~5-10ms | needs `jq` | Good default. Both tools already assume a POSIX env; `jq` is near-ubiquitous on dev machines and easy to check for in `init.sh`. |
| Small Rust/Go binary | ~1-2ms | none at runtime | Best runtime behavior, and we already have a Rust toolchain for the plugin. Costs a second build artifact per platform. |
| `python3` | ~30-50ms | stdlib only | Avoid. Interpreter startup dominates, and it's the slowest option on Codex's 1s `SessionEnd` budget. |

Recommendation: ship the shell script for v1 (milestone 3) since it keeps the install trivially inspectable, then add an optional `zj-agent-mob-hook` Rust binary in milestone 6 built from the same cargo workspace, and have `init.sh` prefer it when present. The transcript-summary extraction is the expensive part, and a native binary can read a bounded tail without shelling out to `jq` at all.

```sh
#!/bin/sh
# Bail fast when not inside a zellij pane -> only current-session agents are monitored.
[ -n "$ZELLIJ_PANE_ID" ] || exit 0

json=$(cat)
eval "$(printf '%s' "$json" | jq -r '
  @sh "event=\(.hook_event_name // "")
       session_id=\(.session_id // "")
       cwd=\(.cwd // "")
       transcript=\(.transcript_path // "")
       tool_name=\(.tool_name // "")"')"
tool="${ZJ_AGENT_TOOL:-claude}"   # init script sets this per hook config

case "$event" in
  SessionStart)                       status=idle ;;
  UserPromptSubmit)                   status=working ;;
  Notification|PermissionRequest)     status=waiting ;;
  PreToolUse|PostToolUse)             status=working ;;
  Stop)                               status=done ;;
  SessionEnd)                         status=ended ;;
  *) exit 0 ;;
esac

# Task summary: only re-extract on turn boundaries, never on tool events (transcripts get large).
task=''
case "$event" in
  SessionStart|UserPromptSubmit|Stop)
    if [ "$tool" = claude ] && [ -n "$transcript" ] && [ -f "$transcript" ]; then
      # ai-title repeats near the end; a bounded tail avoids scanning tens of MB.
      # Must be `tail -n` (lines), NOT `tail -c` (bytes): a byte cut lands mid-line and
      # jq aborts the whole stream on the resulting partial record. Verified failure mode:
      #   jq: parse error: Invalid numeric literal at line 1, column 9
      tail=$(tail -n 300 "$transcript" 2>/dev/null)
      task=$(printf '%s\n' "$tail" | jq -rc 'select(.type=="ai-title") | .aiTitle' 2>/dev/null | tail -1)
      [ -n "$task" ] || task=$(printf '%s\n' "$tail" \
        | jq -rc 'select(.type=="last-prompt") | .lastPrompt' 2>/dev/null | tail -1)
    elif [ "$tool" = codex ] && [ -n "$session_id" ]; then
      roll=$(find "$HOME/.codex/sessions" -name "*-$session_id.jsonl" -type f 2>/dev/null | head -1)
      [ -n "$roll" ] && task=$(jq -r 'select(.type=="event_msg") | .payload
        | select(.type=="user_message") | .message' "$roll" 2>/dev/null | head -1)
    fi
    # Single line, bounded length; commas would break the --args parser.
    task=$(printf '%s' "$task" | tr '\n\r,' '   ' | cut -c1-60 | sed 's/ *$//')
    ;;
esac

zellij pipe --name agent-status \
  --plugin "file:$HOME/.config/zellij/plugins/zj-agent-mob.wasm" \
  --args "pane_id=$ZELLIJ_PANE_ID,tool=$tool,status=$status,session_id=$session_id,cwd=$cwd,task=$task,detail=$tool_name" \
  >/dev/null 2>&1 || true
exit 0
```

Notes:

- Always `exit 0`; never block the agent. Claude hooks are registered with `"async": true`; Codex has no async flag and `SessionEnd` has a 1s default timeout, so the pipe call must be fast (it is - local IPC).
- Codex's legacy `notify` (`agent-turn-complete`, JSON in argv not stdin, kebab-case keys) is a phase-2 fallback; the hooks engine covers `Stop` already.
- `--args` is comma-separated `key=value`, so any comma or newline in a task summary must be stripped (done above). If summaries ever need to be lossless, send them as the pipe *payload* (`-- "$task"`) instead of an arg, since the payload is opaque.
- `PreToolUse`/`PostToolUse` are the activity heartbeat: they keep `status=working` fresh and populate `detail` with the tool name. They also resolve the "permission granted" gap noted in Edge cases, since the next tool event moves `waiting → working`. Registering them roughly doubles hook volume; make them opt-out via `ZJ_AGENT_HEARTBEAT=0`.
- **The plugin must treat an absent/empty `task` arg as "no change", not as "clear the field."** Heartbeat events deliberately send `task=` (empty) to avoid re-reading the transcript, so a naive assignment would blank the summary on every tool call. Same rule for `detail` on turn-boundary events. Concretely: `if let Some(t) = args.get("task").filter(|t| !t.is_empty()) { entry.task = Some(t.clone()) }`.

Verified end-to-end by extracting this script and running it against real transcripts with a stubbed `zellij`:

```console
[SessionStart]      status=idle     task=Review Zellij plugin UI rendering documentation
[UserPromptSubmit]  status=working  task=Review Zellij plugin UI rendering documentation
[Stop]              status=done     task=Review Zellij plugin UI rendering documentation
[PreToolUse]        status=working  task=                      # heartbeat, no transcript read
[Notification]      status=waiting  task=
[SessionEnd]        status=ended    task=
# codex, resolved via ~/.codex/sessions glob on session_id:
[Stop]              status=done     task=PR stack: https://github.com/.../pull/116186
# outside zellij (no $ZELLIJ_PANE_ID): no output, exit 0
```

### 3. Init script (`init.sh`)

**Does this need to be a shell script?** Different answer than for the hook script: here shell is the weaker choice, but it wins anyway for v1.

Startup cost is irrelevant (this runs once), so the deciding factor is JSON merging. The installer has to merge into `~/.claude/settings.json` without clobbering existing hooks, and that's genuinely awkward in shell: it means shelling out to `jq` with a non-trivial reduce, writing to a temp file, and moving it back. A Rust subcommand (`zj-agent-mob init`) gets real `serde_json` parsing, atomic writes, and typed uninstall matching for free.

| Option | JSON merge | Bootstrap cost | Verdict |
|---|---|---|---|
| `sh` + `jq` | awkward but workable | none - runs from a `curl \| sh` install | v1. Users can read it before running it, which matters for a script that edits `~/.claude/settings.json`. |
| `zj-agent-mob init` subcommand | clean, atomic, typed | needs the binary present first | Phase 2. Natural once milestone 6 adds the native hook binary. |
| Standalone Rust installer | clean | second artifact to build/ship | Not worth it as a separate binary. |

Recommendation: `init.sh` in shell for v1, and fold it into the native binary as `zj-agent-mob init` alongside the hook binary in milestone 6. Two hard requirements either way:

- **Atomic writes.** Never edit `~/.claude/settings.json` in place. Write `settings.json.tmp` and `mv` it over, so an interrupted install can't corrupt the user's settings. Back up to `settings.json.bak-<date>` on first run.
- **Idempotence.** Re-running must not duplicate hook entries. Match on the exact command path (`~/.config/zj-agent-mob/hook.sh`) and replace rather than append; that same match drives `init.sh uninstall`.

**Stow-managed settings are a live hazard here.** On this machine both files the installer touches are symlinks into `~/dotfiles`:

```console
$ ls -l ~/.claude/settings.json
... ~/.claude/settings.json -> ../dotfiles/.claude/settings.json
$ ls -l ~/.codex/config.toml
... ~/.codex/config.toml -> ../dotfiles/.codex/config.toml
```

The naive temp-swap (`jq ... > f.tmp && mv f.tmp f`) breaks this, verified with a scratch reproduction: the `mv` replaces the symlink with a regular file, and the edit lands on the *link path* while the real dotfiles copy is left untouched at `{"hooks":{}}`. The user's config silently detaches from the repo and their next `stow -R` reverts the install.

Fix: resolve first, then write through the resolved path.

```sh
target=$(readlink -f "$HOME/.claude/settings.json")
jq '...' "$target" > "$target.tmp" && mv "$target.tmp" "$target"
```

Confirmed this keeps the symlink intact and writes into `~/dotfiles/.claude/settings.json`.

**Decision: writing through into the dotfiles repo is the intended behaviour.** Hooks should be version-controlled and sync across machines. Consequences to design for:

- Resolving through the symlink is the *desired* path, not just a safety measure. Follow the link; do not detach it.
- The install shows up as a `git diff` in `~/dotfiles`. `init.sh` should print the resolved path and a one-line reminder to commit, but must not run `git add`/`commit` itself - that's the user's repo and their call.
- Since hooks sync to other machines, the hook script must degrade silently where the plugin isn't installed. It already does: no `$ZELLIJ_PANE_ID` means exit 0, and `zellij pipe` failures are swallowed by `|| true`. A machine without `zj-agent-mob` sees no errors, just no monitoring.
- The `.wasm` and `hook.sh` themselves go to `~/.config/zellij/plugins/` and `~/.config/zj-agent-mob/`. Whether those are stowed too is the user's choice; the hook path recorded in `settings.json` must match wherever they land, so `init.sh` should accept a `--hook-path` override for a stowed layout.
- Because `settings.json` is shared and version-controlled, the backup (`settings.json.bak-<date>`) matters more, not less: it gives a clean revert that doesn't depend on the repo's commit state. Write it next to the resolved target so it lands in the repo's ignore rules rather than as a stray untracked file - check `~/dotfiles/.gitignore` and add the `.bak-*` pattern if absent.

`~/.codex/hooks.json` does not exist yet, so that one is a clean create. To keep it version-controlled like the others it needs to be created inside `~/dotfiles/.codex/` and stowed, not written directly to `~/.codex/`; `init.sh` should detect that `~/.codex/config.toml` is already a stow symlink and offer the same treatment for `hooks.json`.

Steps:

1. Copy/symlink `zj-agent-mob-hook.sh` to `~/.config/zj-agent-mob/hook.sh`.
2. Copy `zj-agent-mob.wasm` to `~/.config/zellij/plugins/`.
3. **Claude Code**: `jq`-merge into `~/.claude/settings.json` (never clobber existing hooks) entries for `SessionStart`, `UserPromptSubmit`, `Stop`, `SubagentStop` (ignored for status, phase 2), `Notification` (matcher `permission_prompt|idle_prompt`), `SessionEnd` - all `{"type":"command","command":"~/.config/zj-agent-mob/hook.sh","async":true}`.
4. **Codex**: write/merge `~/.codex/hooks.json` (same schema, dedicated file, no TOML surgery) for `SessionStart`, `UserPromptSubmit`, `PermissionRequest`, `Stop`, `SessionEnd`; set `ZJ_AGENT_TOOL=codex` via a small wrapper or hook `args`.
5. Print a suggested Zellij keybind for the user's config:

```kdl
shared_except "locked" {
    bind "Ctrl a" {
        LaunchOrFocusPlugin "file:~/.config/zellij/plugins/zj-agent-mob.wasm" {
            floating true; move_to_focused_tab true
        }
    }
}
```

`init.sh uninstall` removes exactly the entries it added (match on the command path).

## Edge cases

- **Multiple agents in one pane over time**: keyed by `pane_id`; a new `SessionStart` in the same pane replaces the entry.
- **Pane closed / process killed outside the plugin**: reconciled via `PaneUpdate` (entry removed when pane id disappears).
- **Hook fires but plugin never opened**: `zellij pipe --plugin` auto-launches it in the background; first `Ctrl a` shows accumulated state.
- **Subagents (Claude `SubagentStop`, `agent_id` in payload)**: ignored in v1 - pane-level status only.
- **Zellij pipe from a non-pane context** (e.g. `claude` in a plain terminal): guarded by the `$ZELLIJ_PANE_ID` check.
- **Two Zellij sessions at once**: pipes go to the session the pane lives in (the `zellij` CLI inside a pane targets its own session), so each session's plugin instance sees only its own agents.
- **Permission prompt answered**: Claude has no explicit "permission granted" event; the `PreToolUse`/`PostToolUse` heartbeat covers it (`waiting → working` on the next tool event), falling back to `Stop`. With heartbeats disabled, `waiting` persists until the turn ends.
- **Empty task on heartbeat events**: must not clear an existing summary - see hook script notes.
- **Transcript unreadable or `ai-title` absent** (fresh session, or a session too short to have been titled): fall back through `last-prompt` → first user message → `PaneInfo.title`. The task column is `Option<String>`; render the pane title dimmed when `None` so a row is never blank.
- **Codex `transcript_path` is null**: resolve the rollout by globbing `~/.codex/sessions/**/*-$session_id.jsonl`. If the glob misses (custom `CODEX_HOME`), fall back to pane title.
- **Long task summaries / wide-char titles**: truncated to 60 chars at the hook, then ellipsized to fit `cols` at render. Truncate on character boundaries (`chars().take(n)`), not bytes, since summaries can contain non-ASCII.
- **Commas in task summaries**: stripped in the hook, since `--args` is comma-separated. Verified against a real Codex prompt containing a URL followed by `, draft up some...`.

## Milestones

1. **Skeleton** - `cargo init`, `zellij-tile`, `wasm32-wasip1` target, plugin renders a static list, loads via `zellij plugin`, permissions flow works.
2. **Pipe protocol** - `pipe()` handler + hand-run `zellij pipe` commands update the list live.
3. **Hook wiring** - hook script + init script; real Claude Code and Codex sessions appear with correct status transitions. Includes transcript-derived task summaries (`ai-title` / `user_message`) and the empty-arg-means-unchanged upsert rule.
4. **Jump & kill** - selection, `Enter` → `focus_terminal_pane` + `hide_self`, `x`/`X` kill via SIGINT/close, `PaneUpdate` reconciliation.
5. **Polish** - throbber timer, status styling/typography, `done`-vs-`idle` decay, pane-title summary, `show_self` popup on `waiting`, quick-jump `1-9`.
6. **Ship** - README with install one-liner, uninstall, release workflow building the `.wasm` (GitHub Actions, `cargo build --release --target wasm32-wasip1`).
> + PR github workflow


## Non-goals (v1)

- Monitoring agents outside the current Zellij session (explicitly out of scope per requirements).
- Screen-scraping fallback detection (herdr's manifest system) - hooks only. Means agents launched before `init.sh` ran, or tools without hooks (aider, opencode), aren't tracked.
- Desktop notifications beyond terminal bell/OSC.
- Prompting/driving agents from the plugin (herdr's `agent prompt`).

## References

- herdr: https://herdr.dev/docs/ - status model (`working`/`blocked`/`idle`/`done`), hooks-first design
- Claude Code hooks: https://code.claude.com/docs/en/hooks
- Codex hooks: https://learn.chatgpt.com/docs/hooks
- Zellij plugin API: https://zellij.dev/documentation/plugin-api.html, plugin-api-commands.html, plugin-ui-rendering.html
- zellij-tile API docs: https://docs.rs/zellij-tile/latest/zellij_tile/
