# Cross-session rows are inconsistent between panels

Status: proposal. Nothing here is implemented yet.

Reported symptom: with the panel open in several Zellij sessions, **the list is not the same in
each one**. An agent shows up in one panel and not another, and rows linger after the agent is
gone.

This is real. Each panel builds its list from a different mix of sources, and no source is
authoritative, so the panels drift apart and never re-converge.

## Reproducing it

Two live sessions, `web` and `api`, each with the panel open and an agent running.

| Step | Panel in `web` shows | Panel in `api` shows |
|---|---|---|
| Both agents working | both rows | both rows |
| `api`'s agent finishes its turn (`Stop`) | `working` (stale) | `done` |
| `api`'s agent exits (`SessionEnd`) | `working` (stale forever) | row removed |
| Open a *third* panel now | only agents currently running | - |

The third column is the tell: a panel opened later shows a **different, more correct** list than
one that has been open for an hour. Nothing ever reconciles the two.

## Why: three independent mechanisms, each asymmetric

### 1. Status pipes only reach the agent's own session

`scripts/zj-agent-mob-hook.sh:171` sends every status to `zellij pipe --plugin <wasm>` with no
session argument, so it reaches only the plugin in **the session the agent is running in**. This
is the transport gap already recorded as items 10-11 in
[cross-session.md](cross-session.md) - deliberately deferred - but it is the root of the
inconsistency, not just a missing feature.

Consequence: a foreign row in panel `web` is a **snapshot frozen at the moment `web` last heard
anything**, and it never updates again. Whether `web` heard anything at all depends on when its
panel happened to load. Two panels that started at different times hold different snapshots.

### 2. The `ended` event has the same reach as the status it retracts

`state.rs:141` removes a row on `status=ended`. That message travels the same one-session path,
so **only the agent's own panel ever removes the row**. Every other panel keeps it forever.

Verified directly (scratch test, since removed):

```
hook-reported foreign row after empty scan: 1   <- stale forever
```

### 3. The scan culls discovered rows but not hook-reported ones

`apply_scan` (`state.rs:343`) only drops rows whose status is `Discovered`:

```rust
self.agents.retain(|a| {
    a.status != Status::Discovered || found.iter().any(...)
});
```

That guard is correct in a single session - a scan knows less than a hook, so it must not delete a
hook-reported row that it merely failed to see. Across sessions it is exactly backwards: the scan
is the **only** source that sees every session, so it is the only thing that *could* cull a stale
foreign row, and it is forbidden from doing so.

Same scratch test, both directions:

```
discovered foreign row after empty scan:     0   <- culled correctly
hook-reported foreign row after empty scan:  1   <- kept, stale
```

So an agent's row lives forever or not depending on **how that panel first learned about it**,
which is a race between the scan timer and a hook firing.

### Not a cause: `apply_sessions`

Worth stating because it looks suspicious. `apply_sessions` (`state.rs:112`) flips rows to
`unknown` when their session stops being listed. It is consistent across panels - every panel gets
the same `SessionUpdate` - and its `live.is_empty()` guard is correct (an empty list means Zellij
told us nothing, not that every session died). It makes stale rows *visible* as `unknown`, but it
does not create the divergence.

## The fix, in order

The scan is the only cross-session-wide source of truth the plugin has. The fix is to let it act
like one, then close the transport gap.

### 1. Let the scan cull stale foreign rows (small, high value)

A scan sees every session. If it runs successfully and does **not** find a process for a foreign
row, that row is gone - regardless of how it was created. Keep the existing protection for the
panel's own session, where the hook is authoritative and a scan race would cause flicker.

```rust
self.agents.retain(|a| {
    let is_home = a.id.session == self.session_name;
    let seen = found.iter().any(|f| f.pane_id == a.pane_id() && f.session == a.id.session);
    // Home rows: only the scan-discovered ones are the scan's to cull.
    // Foreign rows: the scan is the only thing that can ever cull them.
    if is_home { a.status != Status::Discovered || seen } else { seen }
});
```

Two guards this must not lose:

- **Only on a successful scan.** `plugin.rs:93` already ignores a nonzero exit; keep that, or a
  failed `ps` wipes every foreign row.
- **Not before the first scan completes.** A panel that has piped rows but has never completed a
  scan would cull everything. Gate on a `scan_completed: bool`.

This alone makes every panel converge on the same list within one scan interval, without touching
the hook.

### 2. Make the scan the heartbeat for foreign rows

Even with (1), a foreign row's *status* is a frozen snapshot. Options, in preference order:

**A. Age foreign rows.** A foreign row whose status has not been refreshed in N seconds drops to
`unknown` rather than asserting a stale `working`. Cheap, no new transport, and honest: the panel
genuinely does not know. Pairs naturally with the existing `unknown` status.

**B. Poll foreign sessions for status.** The panel already has `RunCommands`. It could read each
foreign agent's state the way the hook does. This is decision 1's "panel polls" option from
[cross-session.md](cross-session.md#decisions) and is the real fix for live status, but it is a
bigger piece of work and needs a status source the scan does not currently provide.

Recommend A now, B as the follow-up. A is a strict improvement and does not block B.

### 3. Close the transport gap (the pre-existing items 10-11)

Only after 1-2. `ZJ_AGENT_HUB` push or panel-side polling, per the decisions already recorded in
[cross-session.md](cross-session.md). Note the verified hazard there: `zellij --session X pipe`
**blocks** waiting for a plugin reply, so any hook-side push must be backgrounded and timeout-capped
or a closed hub panel stalls every agent on the machine.

## Work items

| # | Item | Notes |
|---|---|---|
| 1 | Add `scan_completed: bool`; set it on the first successful scan | Prerequisite for 2; without it the first scan culls piped rows |
| 2 | Cull stale foreign rows in `apply_scan` regardless of status | The fix. Guard on `scan_completed` and on scan success |
| 3 | Age un-refreshed foreign rows to `unknown` | Stops the panel asserting a stale `working` |
| 4 | Tests: two panels converge; a home row survives a scan race | Below |
| 5 | Document the model in `how-it-works.md` | "who owns which row" is currently implicit |
| 6 | Then revisit transport (cross-session.md items 10-11) | Unchanged, still deferred |

## Tests this needs

The existing suite passes with the bug present, so these are the contract:

- **Convergence.** Two `State`s with different histories - one that learned an agent by pipe, one
  by scan - end up with the same rows after the same scan. This is the reported symptom.
- **A foreign row whose process is gone is culled.** Currently kept forever.
- **A home row is never culled by a scan that missed it.** The existing protection; must not
  regress, and it is what makes the fix non-trivial.
- **No cull before the first successful scan**, and none after a failed one.
- **`ended` still removes the row locally**, unchanged.

## What this does not fix

- Foreign rows still cannot show *live* status until the transport gap closes. They will be
  correct about existence and honest (`unknown`) about state, which is the achievable half.
- Agents in sessions with no attached client remain invisible to `SessionUpdate` - see
  [demo.md](demo.md) trap #15. That is a Zellij property, not this bug.
