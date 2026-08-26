# How it works

- [Status transport](#status-transport)
- [Cross-session status: the spool](#cross-session-status-the-spool)
- [Cost per turn](#cost-per-turn)
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

That scan runs on pane and session events, which is not enough on its own: an agent in another
session can work for ten minutes without opening a pane, and meanwhile the row ages out. So while
any foreign agent is on screen the panel also re-scans every `SPOOL_POLL_INTERVAL` (5s), well
inside the 60s `STALE_AFTER` so a row gets many chances to refresh before it decays. The poll is
gated on a foreign row existing, so a single-session panel never pays for it, and on the session
still being listed, since nothing can refresh a row whose session is gone.

The panel's clock is what paces this, so a foreign row keeps the timer running whatever its status.
An `unknown` row in particular must: it is the row the poll exists to recover, and a panel that
stopped ticking once its rows decayed could never bring them back.

### Urgent transitions skip the poll

A poll cycle is too slow for the states that actually need you, so those are also pushed. Each
open panel touches a beacon file, `panel.<session>`, next to the status records; the hook reads
that directory and, on `waiting` / `failed` / `done` only, additionally runs
`zellij --session <name> pipe` for each panel that is not its own.

The beacon's **filename** is the sanitized session name, which is what compares against the
hook's own `$SESSION`. Its **contents** are the name Zellij actually knows the session by, because
sanitizing is lossy: a session called `my session` is keyed `my_session`, and
`zellij --session my_session` addresses nothing. The same split applies inside the plugin, where
`AgentId.session` is the sanitized key and `real_session()` resolves it back for anything that
takes a `--session` argument.

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
no clock, and this makes a host clock jump unable to pin a row as permanently current. The
reference point is anchored to the panel's tick count, so a record ages as *how far it sat behind
the newest record in its batch, plus how long ago that batch arrived*. Without the second term a
fleet that goes entirely quiet freezes the reference point, and the last record reads as current
forever - which would fight the tick-clock decay and flap the row now that the spool is re-read
every few seconds.

One exception, because the rule above is about records and not about agents: a blocked or idle
agent writes nothing while it sits there, so its record stops advancing and eventually ages out
even though the state is still true. A re-read of an unchanged record therefore re-confirms
`waiting`, `idle-wait`, `idle`, `done` and `failed` - states where silence is exactly what they
predict, and where the process scan still says the agent is alive. It can only ever re-confirm a
status the row already holds, never change one. `working` and `compact` are excluded: they claim
active progress, and silence is evidence against that rather than for it.

`SessionEnd` removes the file. A killed agent fires no `SessionEnd`, so the scan also sweeps
records older than a day - far past `STALE_AFTER`, so they could never render anyway.

The directory is created `0700` and namespaced by uid, because on a shared `/tmp` these records
contain task summaries, which are the user's own prompts. Set `ZJ_AGENT_SPOOL=0` to opt out
entirely; the pipe keeps working for the agent's own session.

## Cost per turn

The hook runs on the agent's critical path, so it is worth knowing exactly what
one turn costs before tuning `ZJ_AGENT_HEARTBEAT` and `ZJ_AGENT_FANOUT`.

**Per hook event, always:**

| Cost | When |
|---|---|
| 1 `jq` | Every event. One invocation parses the whole payload into shell variables via `@sh`. |
| 1 `zellij pipe` | Every event that produces a status or a counter delta. |
| 1 file write | Every event, unless `ZJ_AGENT_SPOOL=0`. Done with a shell redirect, not a subprocess. |

**Only on turn-opening boundaries** (`SessionStart`, `UserPromptSubmit`):

| Cost | Notes |
|---|---|
| 1 `tail -n 300` | Bounded window, so a multi-megabyte transcript is never read whole. |
| 1-2 more `jq` | Claude tries `ai-title` and falls back to `last-prompt`; Codex reads `event_msg` once. |

**Only on `waiting` / `idlewait` / `failed` / `done`** — the states that actually
need you:

| Cost | Notes |
|---|---|
| 1 extra `zellij pipe` **per open panel elsewhere** | Skipped entirely with `ZJ_AGENT_FANOUT=0`. The beacon files are what it counts, so a machine with one panel pays nothing. |

### What dominates, and what to turn off

`PreToolUse` / `PostToolUse` fire **once per tool call**, so in a turn with a
dozen edits they are the whole cost: everything else fires once or twice. That
is why `ZJ_AGENT_HEARTBEAT=0` roughly halves hook volume - it drops both tool
events and the high-frequency counter events (`SubagentStart`, `TaskCreated`)
while leaving turn boundaries, notifications, and permission prompts intact.

The tradeoff is stated in
[troubleshooting](troubleshooting.md#waiting-stays-on-screen-after-youve-answered):
with no heartbeat, `waiting` persists until the turn ends, because Claude has no
"permission granted" event and the next tool call is what would have cleared it.

Rough guide:

- **Default (everything on).** Live status, live tool detail, cross-session
  urgency with no poll delay. Right unless you can measure a problem.
- **`ZJ_AGENT_HEARTBEAT=0`.** Halves the volume. Status still moves at turn
  boundaries; mid-turn tool detail and subagent counters stop updating.
- **`ZJ_AGENT_FANOUT=0`.** Only worth it with several panels open at once.
  Foreign rows fall back to the 5-second spool poll, so urgent states arrive
  late rather than not at all.
- **`ZJ_AGENT_SPOOL=0`.** Drops the file write and opts the agent out of
  cross-session visibility entirely. Its own session's panel is unaffected.

The panel's own cost is separate: `discover::scan_script` runs one `ps axeww`
plus a spool read on every `PaneUpdate`, and every 5 seconds
(`SPOOL_POLL_INTERVAL`) while a foreign row is on screen.

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
