# Cross-session agents

Status: proposal. Nothing here is implemented yet.

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

At most two spawns, constant regardless of session count. Recommended.

The hub is opt-in via `ZJ_AGENT_HUB` in the agent's environment, or a `hub` key in the
plugin's KDL block written into the hook by `init.sh`.

> The verified caveat: `zellij --session X pipe` **blocks** waiting for a plugin reply. If
> the hub session has no panel open, the hook hangs until the pipe is torn down. The hook
> must background this call and cap it (`timeout 2` or `&`), or a closed hub panel stalls
> every agent on the machine. This is the single biggest implementation risk here.

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

Switching sessions detaches the client from the current one. That is a much bigger
context switch than focusing a pane, and it is not obviously reversible - worth a
confirmation step or at least a distinct visual treatment on foreign rows.

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

With agents from several sessions, the row needs to say which. The `project` column
already shows the cwd basename; a foreign agent gets its session name alongside, and
rows group by session with the current one first.

## Work items

Ordered by dependency. Items 1-2 are the correctness prerequisite and should land as
their own change, before anything user-visible.

| # | Item | Blocks | Notes |
|---|---|---|---|
| 1 | Key agents by `(session, pane_id)` | all | Pure refactor, no behavior change while single-session |
| 2 | Session-qualify the verdict file | - | Correctness fix; a collision here answers the wrong prompt |
| 3 | Hook sends `session=` | 4, 6 | One arg; harmless to the current plugin, which ignores unknown args |
| 4 | Discovery scans all sessions | 6 | Delete a filter, add a field. Gets cross-session rows working with no transport change |
| 5 | Keep every `SessionInfo`, not just current | 6, 7 | `src/plugin.rs:67`; needed for foreign tab positions |
| 6 | Render session on foreign rows; group by session | 7 | |
| 7 | Enter switches sessions | - | `switch_session_with_focus` |
| 8 | Hub piping in the hook (`ZJ_AGENT_HUB`), backgrounded | - | Only needed for *live status* of agents in panel-less sessions; item 4 already lists them |
| 9 | Decide kill/approve policy for foreign agents | - | Recommend: refuse, require jump first |

Items 1-7 need no hook transport change at all: discovery alone populates cross-session
rows, and Enter jumps to them. **Item 8 is what upgrades those rows from "discovered" to
live status**, and it is also the riskiest piece. Shipping 1-7 first gives most of the
value with none of the hang risk.

## Open questions

- **Is the hub worth it?** Items 1-7 give a cross-session list where foreign agents show
  as `discovered` (name, session, pane) but not live status. The hub adds live status at
  the cost of a blocking pipe on the hook's hot path. A middle option: the panel polls
  foreign sessions itself via `run_command` on a timer, keeping the risk in the plugin
  where a hang is visible rather than in every agent's hook.
- **Should Enter into another session confirm first?** It detaches the current client.
- **What happens to a row whose session dies?** `SessionUpdate` will stop listing it;
  rows should drop rather than linger pointing at a dead session.

## What this does not address

- Sessions that are not running (`EXITED - attach to resurrect`) have no panes and no
  processes; agents in them are gone, not hidden.
- The plugin still monitors only Claude Code and Codex, and only where the hook or a
  `ps` scan can see them. Remote/SSH agents are out of scope.
