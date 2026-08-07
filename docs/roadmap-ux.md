# Richer agent info: UX sketches and hook feasibility

> [!NOTE]
> **F1-F9 are implemented.** F10 was skipped by decision (Claude sends no model field, so the
> column would be permanently blank for half the rows). This doc is kept as the design record;
> [how-it-works.md](how-it-works.md) documents what actually shipped.

A design doc for pairing. It works in three passes:

1. [What each hook system actually gives us](#1-what-the-hooks-actually-give-us) - the raw material.
2. [UX sketches](#2-ux-sketches) - what the panel could look like, in ASCII.
3. [Prioritisation](#3-prioritisation) - what to build, ordered by value over cost.

The organising constraint: **a feature should work for both Claude Code and Codex**, because a
row that shows rich detail for `claude` and blanks for `codex` reads as a bug. Features that only
one side can support are still listed, but they degrade to the current behaviour rather than
looking broken.

- [1. What the hooks actually give us](#1-what-the-hooks-actually-give-us)
- [2. UX sketches](#2-ux-sketches)
- [3. Prioritisation](#3-prioritisation)
- [4. Open questions](#4-open-questions)

## 1. What the hooks actually give us

Sources: [Claude Code hooks](https://code.claude.com/docs/en/hooks) (31 events),
[Codex hooks](https://learn.chatgpt.com/docs/hooks) (11 events). Verified against both references.

### Event intersection

| Event | Claude | Codex | Wired today | Notes |
|---|:--:|:--:|:--:|---|
| `SessionStart` | yes | yes | yes | Both carry `source` |
| `UserPromptSubmit` | yes | yes | yes | Both carry `prompt` |
| `PreToolUse` | yes | yes | yes | Both carry `tool_input`, `tool_use_id` |
| `PostToolUse` | yes | yes | yes | Codex also has `tool_response` |
| `PermissionRequest` | yes | yes | codex only | Claude has it too; we only wire it for Codex |
| `Stop` | yes | yes | yes | Both carry `last_assistant_message` |
| `SessionEnd` | yes | yes | yes | Both carry `reason` |
| `PreCompact` / `PostCompact` | yes | yes | no | Both carry `trigger` |
| `SubagentStart` / `SubagentStop` | yes | yes | no | Both carry `agent_id`, `agent_type` |
| `Notification` | yes | **no** | yes | Claude-only. `message` + `notification_type` |
| `StopFailure` | yes | **no** | no | Claude-only. Rate limits, billing, auth |
| `PostToolUseFailure` | yes | **no** | no | Claude-only |
| `TaskCreated` / `TaskCompleted` | yes | **no** | no | Claude-only |
| `PermissionDenied` | yes | **no** | no | Claude-only |

Everything else in Claude's 31 (`FileChanged`, `CwdChanged`, `Elicitation`, `WorktreeCreate`,
`ConfigChange`, `InstructionsLoaded`, ...) is Claude-only and not obviously useful for a
pane-level monitor. Left out.

### Field intersection

Common to both, on every event:

```
session_id  transcript_path  cwd  hook_event_name  permission_mode
```

Codex additionally sends `model` (active model slug) and `turn_id` on every turn-scoped event.
**Claude sends neither.** No hook payload on either side carries token counts, cost, or
context-window usage.

Per-event fields we do not currently read but both tools send:

| Field | Event | Today |
|---|---|---|
| `tool_input` | `PreToolUse`, `PostToolUse`, `PermissionRequest` | discarded; we send only `tool_name` |
| `last_assistant_message` | `Stop` | discarded; we tail the transcript instead |
| `prompt` | `UserPromptSubmit` | discarded; we tail the transcript instead |
| `source` | `SessionStart` | discarded |
| `reason` | `SessionEnd` | discarded |
| `permission_mode` | most events | discarded |
| `agent_id`, `agent_type` | `SubagentStart/Stop` | event not wired |
| `trigger` | `PreCompact/PostCompact` | event not wired |

Note the second and third rows: `Stop.last_assistant_message` and `UserPromptSubmit.prompt` are
handed to us directly, while the hook currently does `tail -n 300` plus two `jq` passes over a
multi-megabyte transcript to get worse versions of the same thing. Adopting them **removes**
code.

### Hook output protocol

Both tools accept the same stdout JSON: `decision`, `permissionDecision`, `additionalContext`,
`systemMessage`, `continue`, `stopReason`. This is what makes [act-on-permission](#f9) possible.

Codex caveat: `async` is parsed but not implemented, and `SessionEnd` has a **1 second** timeout.
Any blocking design must assume the hook is synchronous and on the critical path of the turn.

## 2. UX sketches

Today, for reference:

```
zj-agent-mob   1 waiting · 1 working · 1 done

▶ 1 ● codex   waiting    2s  web        Fix flaky checkout test
      └ needs approval: rm -rf node_modules · pane:5
  2 ✓ claude  done       2s  dotfiles   Review zellij plugin docs
      └ pane:7
  3 ⠙ claude  working    2s  api        Add retry to webhook client
      └ Edit src/webhook.rs · 1 turns · pane:9
  4 ○ codex   idle       2s  cli        Bump deps
      └ pane:11

 ↵ jump   1-9 quick   x kill   d dismiss   i install   q hide
```

Two lines in that screenshot are aspirational: `needs approval: rm -rf node_modules` and
`Edit src/webhook.rs`. The hook sends `detail=$tool_name`, so the real panel shows `Bash` and
`Edit`. F1 and F2 below make the screenshot honest.

---

### F1. Tool detail with arguments

`Edit` becomes `Edit src/webhook.rs`. `Bash` becomes the command.

```
  3 ⠙ claude  working    2s  api        Add retry to webhook client
      └ Edit src/webhook.rs · 1 turns · pane:9

  3 ⠙ claude  working   14s  api        Add retry to webhook client
      └ Bash cargo test --release · 1 turns · pane:9
```

Both tools, `tool_input.file_path // .tool_input.command // .tool_input.pattern`.
Hook-only change: `detail` is already a free-form passthrough string.

---

### F2. Real notification text

Claude's `Notification` carries `message` and `notification_type`. Today both
`permission_prompt` and `idle_prompt` collapse into `waiting`, but they are different problems:
one wants your approval, the other means the agent has been abandoned.

```
▶ 1 ● codex   waiting    2s  web        Fix flaky checkout test
      └ needs approval: rm -rf node_modules · pane:5

  5 ◐ claude  idle-wait  4m  infra      Terraform plan review
      └ waiting for you (4m) · pane:13
```

Claude gets this from `Notification`. **Codex has no `Notification` event**, so Codex gets the
approval line from `PermissionRequest.tool_input` (already wired) and simply never shows the
`idle-wait` variant. Degrades cleanly.

---

### F3. Final message on done

A `done` row currently shows the task summary from *before* the turn. `Stop.last_assistant_message`
says what actually happened.

```
  2 ✓ claude  done      2s  dotfiles   Review zellij plugin docs
      └ Found 3 issues in the render path · pane:7
```

Both tools. Removes the transcript tail on `Stop`.

---

### F4. Failure state

Claude's `StopFailure` fires on `rate_limit`, `overloaded`, `billing_error`,
`authentication_failed`, `max_output_tokens`. Today a rate-limited agent sits at `working`
forever and looks healthy - the single most misleading thing the panel currently does.

```
zj-agent-mob   1 failed · 1 waiting · 1 working

▶ 1 ✗ claude  failed    31s  api        Add retry to webhook client
      └ rate limited · retry in ~30s · pane:9
```

Needs a fifth `Status` variant with error colouring, sorted above `waiting`.

**Claude-only.** Codex has no error event. A Codex agent that dies this way still shows the
current (wrong) behaviour. This is the strongest argument for building it anyway: the failure
mode is bad enough that fixing it for one tool beats fixing it for neither.

---

### F5. Subagent fan-out

A fan-out currently shows one silent `working` row for minutes. `SubagentStart`/`SubagentStop`
give `agent_type` and `agent_id`.

```
  3 ⠙ claude  working   1m12s  api      Audit the auth layer
      └ 3 subagents: Explore, Plan, code-reviewer · pane:9
```

Expanded, if we add a detail view:

```
  3 ⠙ claude  working   1m12s  api      Audit the auth layer
      ├ ⠙ Explore          42s   scanning src/auth
      ├ ✓ Plan             1m03s done
      └ ⠙ code-reviewer    12s   Read src/auth/token.rs
```

Both tools. Needs a nested child model on `Agent` and a per-row expand key.

---

### F6. Compaction

Compaction is a multi-second freeze that reads as a hang.

```
  3 ⠸ claude  compact   6s   api        Add retry to webhook client
      └ compacting context (auto) · pane:9
```

Both tools, `trigger` distinguishes `manual` from `auto`. Cheap: could be a transient
`detail` string rather than a real status.

---

### F7. Permission mode badge

`plan`, `acceptEdits`, `bypassPermissions` are materially different risk postures. An agent
running unattended in `bypassPermissions` is exactly what a monitoring panel exists to surface.

```
  3 ⠙ claude  working   2s  api    [bypass]  Add retry to webhook client
  6 ⠙ claude  working   9s  web    [plan]    Design the migration
```

Both tools. Only render the badge when it is not `default`, so the common case stays clean.

---

### F8. Native task progress

Claude's `TaskCreated`/`TaskCompleted` give `task_id` and `task_title`. Better than the current
`turns` counter, which counts heartbeat status transitions and means little.

```
  3 ⠙ claude  working   2s  api        Add retry to webhook client
      └ Edit src/webhook.rs · 4/7 tasks · pane:9
```

**Claude-only.** Falls back to `turns` for Codex.

---

### F9. Approve or deny from the panel

Both tools accept `permissionDecision: allow|deny` on stdout from `PermissionRequest`. This is
the highest-leverage action for a mob-of-agents workflow: clear a prompt without leaving the
panel.

```
▶ 1 ● codex   waiting    2s  web        Fix flaky checkout test
      └ needs approval: rm -rf node_modules · pane:5
        ┌──────────────────────────────────────────┐
        │  Bash                                    │
        │  rm -rf node_modules                     │
        │                                          │
        │  a approve    d deny    ↵ jump to pane   │
        └──────────────────────────────────────────┘

 ↵ jump   a approve   d deny   x kill   q hide
```

Both tools support the protocol, but this is the one genuinely invasive item. The hook must
block while the plugin pipes a verdict back, and Codex has no working `async`. A hook that
hangs wedges the turn.

Constraints if we build it:
- Config flag, default off.
- Short timeout that falls through to the normal in-pane prompt.
- Separate spike, not bundled with anything else.
- `d` currently means "dismiss" - it would need rebinding to avoid a mis-keyed `deny`.

---

### F10. Model badge

Codex sends `model` on every turn-scoped event. **Claude sends no model field on any hook.**

```
  4 ○ codex   idle      2s  cli   gpt-5-codex   Bump deps
  3 ⠙ claude  working   2s  api                 Add retry to webhook client
```

An asymmetric column that is permanently blank for half the rows. Listed for completeness;
recommend skipping unless Claude adds the field.

## 3. Prioritisation

Cost is implementation surface: **hook** = shell only, **hook+ui** = also Rust.

| # | Feature | Both tools | Cost | Value | Status |
|---|---|:--:|---|---|---|
| F1 | Tool detail with args | yes | hook | high | **Shipped** |
| F3 | Final message on done | yes | hook | high | **Shipped** |
| F2 | Real notification text | claude+partial | hook | high | **Shipped** |
| F4 | Failure state | claude only | hook+ui | very high | **Shipped** |
| F7 | Permission mode badge | yes | hook+ui | medium | **Shipped** |
| F6 | Compaction | yes | hook | medium | **Shipped** |
| F5 | Subagent fan-out | yes | hook+ui | high | **Shipped** (flat count) |
| F8 | Native task progress | claude only | hook+ui | medium | **Shipped** |
| F9 | Approve from panel | yes | hook+ui+protocol | very high | **Shipped** (opt-in) |
| F10 | Model badge | codex only | hook+ui | low | Skipped by decision |

### Suggested first slice

F1, F3, F2 are pure `scripts/zj-agent-mob-hook.sh` edits. No Rust, no new events on the Codex
side, no schema change - `task` and `detail` are already free-form passthrough strings, and the
plugin's "empty means unchanged" rule keeps heartbeats from clobbering them. F3 deletes the
`Stop` transcript tail.

Then F4 as the first UI change, since a rate-limited agent showing `working` is the panel's
worst current lie.

### Cost caution

`SubagentStart/Stop` (F5) and `TaskCreated/Completed` (F8) are high-frequency. The existing
`ZJ_AGENT_HEARTBEAT=0` escape hatch only guards `PreToolUse`/`PostToolUse`. Extend that gating
before wiring either, or the hook fires several times per second during a fan-out - on Codex
that lands on the turn's critical path, since `async` is not implemented.

## 4. Open questions

Resolved during implementation:

- **F4 asymmetry.** Built. A Codex agent that dies to a rate limit still shows the old (wrong)
  `working`, but fixing it for one tool beat fixing it for neither.
- **F5 nesting.** Flat count on the detail line (`3 subagents: Explore, Plan`), not child rows.
  Keeps the one-row-per-pane model, which the whole pane-reconciliation path depends on.
  Per-subagent rows remain possible later.
- **F9 key binding.** Reject is <kbd>r</kbd>, not <kbd>d</kbd>. `d` stays dismiss, so a mis-keyed
  dismiss can never answer a permission prompt. Approve/reject only appear in the footer while a
  prompt is actually parked.
- **Idle-wait threshold.** Trusts Claude's own `idle_prompt` schedule rather than adding a second
  timer. One less thing to tune, and the agent knows better than the panel does.

Still open:

- **`turn_id`.** Codex sends it, Claude does not. Would give an exact turn count instead of the
  inferred `turns`, but only for one tool.
- **F9 on Codex.** The protocol is supported and the code path is shared, but it is untested
  against a real Codex binary - Codex parses `async` without implementing it, so a slow verdict
  sits on the turn's critical path. The timeout bounds it; real-world feel is unverified.
- **Multiple queued prompts.** One parked prompt per pane. A second replaces the first, which is
  right for a single agent but untested against an agent that prompts twice in quick succession.
