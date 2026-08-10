# How it works

- [Status transport](#status-transport)
- [Cross-session status: the spool](#cross-session-status-the-spool)
- [Task summaries](#task-summaries)
- [Counter events](#counter-events)
- [Answering permission prompts](#answering-permission-prompts)
- [The install screen](#the-install-screen)

## Status transport

Each hook event runs `scripts/zj-agent-mob-hook.sh`, which forwards status to the plugin:

```sh
zellij pipe --name agent-status --plugin file:...wasm \
  --args "pane_id=$ZELLIJ_PANE_ID,tool=claude,status=waiting,..."
```

`$ZELLIJ_PANE_ID` is set by Zellij in every terminal pane and is inherited by processes started there, so it identifies the exact pane. If it isn't set, the hook exits immediately, which is what scopes monitoring to the current session: agents outside Zellij are ignored, and each session's plugin instance only sees its own panes.

`zellij pipe --plugin` auto-launches the plugin if it isn't running, so there's no daemon and no socket.

## Cross-session status: the spool

The pipe above reaches only the plugin in the agent's *own* session. To give a panel live status
for agents everywhere, the hook also writes one file per agent:

```
$TMPDIR/zj-agent-mob-<uid>/status/<session>.<pane_id>
```

One line, the same `key=value` payload plus a `ts=`. The write is a `printf` to a `.tmp` and a
`mv` into place - no subprocess, nothing to block on, and `rename(2)` is atomic within a
filesystem, so a reader sees the old record or the new one but never a half-written one. That
matters because this runs on the critical path of every tool call.

The panel reads the directory on the same `run_command` that already runs the process scan, so
polling costs one command rather than two.

### Urgent transitions skip the poll

A poll cycle is too slow for the states that actually need you, so those are also pushed. Each
open panel touches a beacon file, `panel.<session>`, next to the status records; the hook reads
that directory and, on `waiting` / `failed` / `done` only, additionally runs
`zellij --session <name> pipe` for each panel that is not its own.

The restriction to those four statuses is the cost control: tool events fire constantly, and
they must never pay for a subprocess per open panel. Heartbeats take the spool path alone. A
beacon older than five minutes is swept, so a closed panel stops attracting pipes. Set
`ZJ_AGENT_FANOUT=0` to opt out; everything falls back to the poll.

## Notifications

The plugin, not the hook, decides when to notify: it holds every agent's state, so it can
debounce across the fleet where a per-agent hook could not. A transition into a notifying status
is queued rather than sent, and the queue is flushed after a short window, which turns a fan-out
of five blocked agents into one banner instead of five stacked ones.

Three rules keep it from becoming noise: one notification per agent per `notify_cooldown`,
nothing at all while the panel is already on screen, and only the newest state per agent inside a
window. The notifier binary is probed once via `run_command` (`terminal-notifier`, then
`osascript`, then `notify-send`) and cached; finding none disables the feature silently.

Task summaries and tool arguments come from arbitrary repo content, so every one is passed as its
own argv element. `osascript` has no argv form for `display notification`, so the text is bound
through `on run argv` rather than spliced into the script.

### Who owns which row

Three sources can say something about an agent, and they are deliberately not equal:

| Source | Owns | Beats |
|---|---|---|
| Pipe (the agent's own session) | status for home rows | everything, for home rows |
| Spool | status for foreign rows | the scan |
| Process scan | whether a row exists at all | nothing; it asserts existence only |

**The rule that makes this safe: a spool record never creates a row.** Existence comes from the
process scan; the spool only refines a row the scan already justified. So a leftover file cannot
resurrect an agent that exited, and losing the spool degrades to the pre-existing behaviour
(`found` rows) rather than breaking anything.

A home row is skipped by the spool merge entirely: its hook pipes straight into this session, so
the pipe is both fresher and authoritative, and letting a poll overwrite it would flap the row.

Four defences keep a stale record from showing wrong data:

| Defence | Stops |
|---|---|
| No process, no row | A record for an agent that has exited |
| `session_id` must match | A recycled pane id inheriting the previous agent's status |
| `ts` older than `STALE_AFTER` is ignored | A record from a previous boot or a long-idle agent |
| Filename must match the record's own `session`/`pane_id` | A malformed or mislabelled file |

Records are dated relative to the newest one seen rather than against a wall clock: the plugin has
no clock, and this makes a host clock jump unable to pin a row as permanently current.

`SessionEnd` removes the file. A killed agent fires no `SessionEnd`, so the scan also sweeps
records older than a day - far past `STALE_AFTER`, so they could never render anyway.

The directory is created `0700` and namespaced by uid, because on a shared `/tmp` these records
contain task summaries, which are the user's own prompts. Set `ZJ_AGENT_SPOOL=0` to opt out
entirely; the pipe keeps working for the agent's own session.

## Task summaries

- **Claude Code** transcripts contain an `ai-title` record: Claude's own rolling summary of the session (e.g. "Review Zellij plugin UI rendering documentation"). Falls back to `last-prompt`, then the pane title.
- **Codex** has no equivalent, so the first `event_msg` / `user_message` record from the session rollout is used. Raw `response_item` records are skipped because they're padded with `AGENTS.md` and `<INSTRUCTIONS>` preamble.

Transcripts reach tens of megabytes, so summaries are re-read only on turn-opening boundaries (`SessionStart`, `UserPromptSubmit`) from a bounded `tail -n 300` window. Tool events send an empty `task=`, and the plugin treats empty as "leave unchanged".

`Stop` needs no transcript read at all: both agents put the turn's closing text in `last_assistant_message`, so a finished row shows what actually happened rather than the summary from before the turn.

## Counter events

`SubagentStart` / `SubagentStop` and `TaskCreated` / `TaskCompleted` all fire on the parent's pane, so they cannot carry a status without overwriting the pane's own state. They send an empty `status=` plus a delta (`subagent_delta=1`, `task_done_delta=1`) instead, and the plugin applies it to the existing row. Deltas rather than absolute counts, because the hook is stateless: each event only knows that one more thing started or finished. Counters reset when a new turn begins, and saturate at zero so a stray `Stop` cannot underflow.

These events are high-frequency during a fan-out, so they honour `ZJ_AGENT_HEARTBEAT=0` alongside the tool events.

## Answering permission prompts

Opt-in via `ZJ_AGENT_APPROVE=1`, and the only hook that deliberately blocks a turn. `PermissionRequest` is registered with `async: false` for exactly this reason; every other Claude hook stays async so it can never stall the agent.

The plugin cannot write to the stdin of an already-running hook, so the verdict travels through a file:

1. The hook pipes `agent-ask` with a `verdict_file` path and blocks, polling for it.
2. <kbd>a</kbd> / <kbd>r</kbd> in the panel writes `allow` or `deny` to that path via `run_command`.
3. The hook reads it and prints `{"hookSpecificOutput":{"decision":{"behavior":"allow"}}}`.

Polling rather than a FIFO: opening a FIFO with no reader blocks past any timeout, and Codex parses `async` but does not implement it, so there is nothing to absorb a hang. On timeout the hook prints nothing and exits 0, which falls through to the agent's own prompt - the worst case is the normal interactive experience, never a wedged turn.

The path is passed to `sh -c` as a positional argument rather than interpolated into the command string, so nothing outside the plugin is ever parsed by a shell.

`/goal` sets a session-scoped hook but leaves no on-disk artifact, so goals aren't readable. `ai-title` tracks the real work anyway. For a manual override:

```sh
zellij pipe --name agent-label --args "pane_id=3,label=whatever you want"
```

## The install screen

The plugin runs in WASI with no access to `$HOME`, so it cannot read `settings.json` to see whether hooks are installed. Instead it shells out via Zellij's `run_command` to `~/.config/zj-agent-mob/install.sh status`, which prints one `key=state` line per target:

```text
claude=installed
codex=absent
plugin=installed
hook=installed
```

Results arrive asynchronously as a `RunCommandResult` event, tagged with a context key so install output is never confused with another command's. After any toggle the plugin re-reads state rather than assuming the change landed, because the installer can succeed partially (hooks written, plugin not built).
