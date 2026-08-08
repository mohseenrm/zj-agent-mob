# Cross-session agents

Status: **implemented** (items 1-9 and 12). Items 10-11, the live-status transport
for agents in sessions with no panel open, remain open - see Decisions below.

Today the panel shows agents in the session it is running in. The goal: show every
Claude Code and Codex agent across every live Zellij session, and have <kbd>Enter</kbd>
drop you into the right session *and* the right pane.

## Why this is mostly already possible

Four things were verified against Zellij 0.44.3 before writing this, because each one
could have killed the design:

| Question | Answer |
|---|---|
| Can a plugin switch sessions and focus a pane in one step? | **Yes.** `switch_session_with_focus(name, tab_position, pane_id)` (`zellij-tile-0.44.3/src/shim.rs:1500`) |
| Can a plugin see other sessions' panes? | **Yes.** `SessionUpdate` delivers `Vec<SessionInfo>`, each with a full `PaneManifest` (`data.rs:1784`) |
| Can a hook in session A reach a plugin in session B? | **Yes.** `zellij --session <name> pipe ...` connects to a named session |
| Does the plugin already receive the data? | **Yes.** `src/plugin.rs:67` gets every session on `SessionUpdate` and discards all but the current one |

So the transport and the jump both exist. The work is mostly about *identity* and
*scoping*, not about new Zellij capabilities.

## The one real blocker: agents are keyed by pane id alone

`Agent.pane_id: u32` (`src/agent.rs:10`) is the identity of a row. `reconcile`, the kill
path, the verdict file, and the jump all key off it.

Pane ids are **only unique within a session**. Two sessions each having a pane 3 is
normal. Every one of these breaks the moment two sessions report:

- Two agents collapse into one row, or flap between two states.
- <kbd>x</kbd> kills a same-numbered pane in the wrong session.
- The permission verdict file (`verdict.$ZELLIJ_PANE_ID`) collides, so approving one
  agent can answer another's prompt.

That last one is the dangerous one: it silently approves a tool call in a session you
were not looking at. **Nothing else in this plan should land before the key change.**

The fix is to key by `(session, pane_id)` everywhere:

```rust
pub(crate) struct AgentId {
    pub(crate) session: String,
    pub(crate) pane_id: u32,
}
```

`session` is a `String` rather than an index because session ids are not stable across
restarts and the name is what `switch_session_with_focus` takes.

## Design

### 1. The hook reports which session it is in

`ZELLIJ_SESSION_NAME` is already in the agent's environment - `src/discover.rs:40` reads
it out of `ps` today. The hook adds it to the pipe args:

```sh
--args "pane_id=$ZELLIJ_PANE_ID,session=$ZELLIJ_SESSION_NAME,tool=$TOOL,..."
```

One field, and it makes every message self-identifying.

### 2. The hook pipes to a chosen session, not its own

This is the crux of the transport. `zellij pipe` with no `--session` targets the session
the hook runs in, so an agent in session B reaches only a plugin in session B.

Two options, and the choice is worth making explicitly:

**A. Fan out - every hook pipes to every live session.** The hook enumerates
`zellij list-sessions` and pipes N times. Every open panel sees every agent, no
configuration. Cost: N subprocesses per hook event, on the hot path of every tool call.
With 6 sessions that is 6 `zellij` spawns per keystroke-ish event. Rejected: the hook is
`async: true` precisely so it can never slow a turn, and this reintroduces that risk.

**B. One designated hub session.** The hook pipes to its own session (so a local panel
keeps working exactly as now) plus one configured hub if set:

```sh
zellij pipe --name agent-status --plugin "$PLUGIN" --args "..." 
[ -n "$ZJ_AGENT_HUB" ] && [ "$ZJ_AGENT_HUB" != "$ZELLIJ_SESSION_NAME" ] &&
  zellij --session "$ZJ_AGENT_HUB" pipe --name agent-status --plugin "$PLUGIN" --args "..."
```

At most two spawns, constant regardless of session count.

The hub is opt-in via `ZJ_AGENT_HUB` in the agent's environment, or a `hub` key in the
plugin's KDL block written into the hook by `init.sh`.

> The verified caveat: `zellij --session X pipe` **blocks** waiting for a plugin reply. If
> the hub session has no panel open, the hook hangs until the pipe is torn down. The hook
> must background this call and cap it (`timeout 2` or `&`), or a closed hub panel stalls
> every agent on the machine. This is the single biggest implementation risk here.

**C. The panel polls (chosen - see decision 1).** Instead of agents pushing to a hub, the
panel runs a periodic `run_command` that collects status for foreign sessions. Same live
status, but the blocking risk lives inside one plugin the user is watching rather than on
every agent's hot path. B stays available as opt-in for push latency.

### 3. Discovery scans all sessions

`scan_script` (`src/discover.rs:33`) already parses `ZELLIJ_SESSION_NAME` out of every
process environment and then filters to one session:

```awk
if (pane != "" && sess == want) print pane, cmd
```

Drop the filter, print the session, and one `ps` covers every session at once. This is
strictly less work than today, and it means agents in sessions with no panel open still
appear - the discovery path needs no hub and no cross-session pipe at all.

```awk
if (pane != "" && sess != "") print sess, pane, cmd
```

### 4. Enter jumps across sessions

`handle_key` (`src/keys.rs:112`) currently calls `focus_terminal_pane` unconditionally.
It becomes a branch:

```rust
if agent.id.session == self.session_name {
    host::focus_terminal_pane(agent.id.pane_id, true, false);  // unchanged
    host::hide_self();
} else {
    // Tab position comes from the SessionInfo manifest; None is acceptable
    // (Zellij focuses the pane's own tab) but passing it avoids a visible
    // tab flash on switch.
    host::switch_session_with_focus(&agent.id.session, agent.tab, Some((agent.id.pane_id, false)));
}
```

The `bool` in `pane_id: Option<(u32, bool)>` is `is_plugin`; agents are terminal panes, so
`false`.

Switching sessions detaches the client from the current one. Per decision 2 this happens
immediately, with no confirmation: jumping is the primary action of the panel and a
prompt on every jump costs more than the occasional accidental detach. Foreign rows carry
a distinct visual treatment (item 6) so the switch is never a surprise.

For a row whose session has exited, `switch_session_with_focus` would resurrect it and
land on a pane with no agent in it. Those rows attach the session instead and skip the
pane focus - see decision 3.

### 5. Kill and approve become session-aware

- `send_sigint_to_pane_id` / `close_terminal_pane` take a bare pane id and act on the
  *current* session. There is no cross-session equivalent in the plugin API. For a
  foreign agent, either shell out (`zellij --session X action ...`, needs `RunCommands`)
  or refuse and require jumping first. **Refusing is the safer default** given the kill
  path is already two-step for a reason.
- The verdict file must include the session: `verdict.$ZELLIJ_SESSION_NAME.$ZELLIJ_PANE_ID`.
  This is a hook change and a `src/host.rs:23` change, and it is the correctness fix
  called out above.

### 6. The list shows where each agent lives

With agents from several sessions, the row needs to say which. A foreign row shows its
session name *in place of* the project column, dimmed, so no column shifts and the width
contract holds.

Rows are **not** grouped by session. The existing sort puts whatever needs you most at
the top, and that matters more than locality: an agent blocked on a permission prompt in
another session is exactly the one you want at row 1.

## Work items

Ordered by dependency. Items 1-2 are the correctness prerequisite and should land as
their own change, before anything user-visible.

| # | Item | Blocks | Notes |
|---|---|---|---|
| 1&nbsp;✅ | Key agents by `(session, pane_id)` | all | Pure refactor, no behavior change while single-session |
| 2&nbsp;✅ | Session-qualify the verdict file | - | Correctness fix; a collision here answers the wrong prompt |
| 3&nbsp;✅ | Hook sends `session=` | 4, 6 | One arg; harmless to the current plugin, which ignores unknown args |
| 4&nbsp;✅ | Discovery scans all sessions | 6 | Delete a filter, add a field. Gets cross-session rows working with no transport change |
| 5&nbsp;✅ | Keep every `SessionInfo`, not just current | 6, 7 | `src/plugin.rs:67`; needed for foreign tab positions |
| 6&nbsp;✅ | Render session on foreign rows | 7 | Foreign rows show their session in place of the project column. Not grouped by session: the existing needs-attention-first sort is more useful than locality |
| 7&nbsp;✅ | Enter switches sessions, no confirmation | - | `switch_session_with_focus` (decision 2) |
| 8&nbsp;✅ | `Status::Unknown`; rows persist when a session stops being listed | 9 | Decision 3 |
| 9&nbsp;✅ | Enter on a dead session resurrects rather than focuses; kill refused there | - | Decision 3's correction |
| 10 | Foreign-session polling on a timer (`run_command`) | - | Decision 1, preferred form |
| 11 | Optional `ZJ_AGENT_HUB` push, backgrounded and timeout-capped | - | Decision 1, opt-in; must never block the hook |
| 12&nbsp;✅ | Kill/approve policy for foreign agents: refuse, require jump first | - | Unchanged recommendation |

✅ = shipped. Items 1-9 and 12 are in. They need no hook transport change: discovery
alone populates cross-session rows, and Enter jumps to them. **Items 10-11 are what
upgrade those rows from `found` to live status**, and they carry the hang risk described
above, so they were deliberately left out of this pass.

## Decisions

All three answered. Recorded here because each one changes the work items above.

### 1. The hub ships. Yes.

Live status for agents in panel-less sessions is worth the transport. Item 8 is in scope,
with the mandatory backgrounding from the caveat above - the hook must never block on a
hub that has no panel open.

**Prefer the middle option**: the panel polls foreign sessions on a timer via
`run_command` rather than every hook pushing to the hub. Same result, but a hang shows up
in one plugin the user is looking at instead of stalling every agent on the machine. The
`ZJ_AGENT_HUB` push stays available as opt-in for anyone who wants push latency over
poll safety.

### 2. Enter switches immediately. No confirmation.

Jumping is the primary action and a prompt on every jump would be worse than the
occasional accidental detach. Foreign rows still get a distinct visual treatment (item 6)
so the switch is never a surprise, but nothing blocks it.

### 3. A dead agent goes to `unknown`, it does not disappear.

Rows persist when their session stops being listed, moving to an `unknown` status rather
than vanishing. An agent silently dropping off the list is worse than a stale row: the
user cannot tell whether it finished, crashed, or was never there.

> **One correction to the stated intent.** The answer asks that the user still be able to
> jump to or kill an `unknown` agent. When a Zellij session has genuinely exited there is
> no pane to focus and no process to signal - `list-sessions` shows those as
> `EXITED - attach to resurrect`, and their panes and processes are gone. Kill on such a
> row cannot do anything, and `switch_session_with_focus` into a dead session resurrects
> it to a pane that no longer holds an agent.
>
> The distinction that makes the request work is **why** the row went `unknown`:
>
> - **Session still alive, agent gone** (process exited, pane closed): jump works, kill is
>   a no-op. Keep both enabled.
> - **Session itself exited**: nothing to jump to. Enter should offer to *resurrect* the
>   session (`switch_session` does attach a dead session) rather than pretend to focus a
>   pane, and kill should be disabled with the reason shown.
>
> So: rows persist as `unknown` in both cases, per the decision. Enter stays enabled but
> means "resurrect" rather than "focus" for a dead session, and kill is refused there
> because there is no process. Flagging because "jump to pane or kill it" is not
> achievable as written for the exited case, and silently doing nothing would be worse
> than saying why.

`Status::Unknown` is a new variant alongside the existing `Discovered` (`src/status.rs:14`),
which is also scan-produced and never arrives from a hook.

## Known bug

Panels in different sessions do not agree on the list: a row can appear in one and not another,
and foreign rows linger after the agent is gone. Root cause and fix are in
[cross-session-consistency.md](cross-session-consistency.md). Items 10-11 below are part of the
story but not the whole of it - two of the three mechanisms are in the plugin, not the transport.

## What this does not address

- Sessions that are not running (`EXITED - attach to resurrect`) have no panes and no
  processes; agents in them are gone, not hidden. Their rows persist as `unknown` per
  decision 3, but the panel cannot report what those agents were doing when the session
  died - only that they were there.
- The plugin still monitors only Claude Code and Codex, and only where the hook or a
  `ps` scan can see them. Remote/SSH agents are out of scope.
