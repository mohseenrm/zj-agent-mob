# Roadmap: deeper hook integration

An inventory of the hook surface we consume today versus what the hooks contract
actually offers (https://learn.chatgpt.com/docs/hooks), and ranked proposals for
the unused parts. Companion to [roadmap-next.md](roadmap-next.md), which ranks
user-facing features; this ranks *transport-level* capability we are leaving on
the table.

- [What we consume today](#what-we-consume-today)
- [Unused surface, inventoried](#unused-surface-inventoried)
- [Proposals](#proposals)
- [What not to build](#what-not-to-build)
- [Sequencing](#sequencing)
- [Build notes](#build-notes)

> **Status: H1-H7 are built and shipped.** Each proposal below keeps its original
> text; where the build deviated from the plan, a **Built** note says how and
> why. [Build notes](#build-notes) collects what only showed up in the doing.

## What we consume today

The hook script subscribes to nearly every lifecycle event and maps each to a
status (see [how-it-works.md](how-it-works.md#status-transport)):

| Consumed | Used for |
|---|---|
| `SessionStart` / `SessionEnd` | row birth / retirement |
| `UserPromptSubmit`, `Stop`, `StopFailure` | turn boundaries, `done` / `failed` |
| `PreToolUse` / `PostToolUse` / `PostToolUseFailure` | heartbeat + tool detail line |
| `Notification`, `PermissionRequest` | `waiting` / `idlewait` + block reason |
| `PreCompact` / `PostCompact` | `compact` status |
| `SubagentStart/Stop`, `TaskCreated/Completed` | counter deltas |
| Input fields | `hook_event_name`, `session_id`, `cwd`, `transcript_path`, `tool_name`, `tool_input.*`, `last_assistant_message`, `message`, `notification_type`, `permission_mode`, `agent_type`, `error_*`, `trigger` |
| Output contract | `PermissionRequest` decision (`allow`/`deny`), opt-in via `ZJ_AGENT_APPROVE` |
| Config | user-level `settings.json` / `hooks.json`, `async: true` everywhere except `PermissionRequest`, `Notification` matcher scoping |

We are read-heavy and write-shy: of everything a hook is allowed to *say back*
to the agent, we use exactly one decision on one event. Everything below is
about the other direction.

## Unused surface, inventoried

| Capability | Contract | Today |
|---|---|---|
| `Interrupt` event | fires when the user interrupts a turn (Esc) | falls through `*) exit 0` - **a real gap, see H1** |
| `tool_use_id` | correlates `PreToolUse` with its `PostToolUse` | ignored |
| `tool_response` on `PostToolUse` | the tool's actual result | ignored |
| `turn_id` | groups all events of one turn | ignored |
| `model` | which model the session runs | ignored |
| `Stop` → `decision: "block"` + prompt text | forces the agent to continue with injected instructions | unused |
| `additionalContext` (SessionStart, UserPromptSubmit, PostToolUse) | inject up to ~2500 tokens of context into the model | unused |
| `updatedInput` on `PreToolUse` | rewrite a tool call before it runs | unused (deliberately, see below) |
| `permissionDecision` beyond allow/deny | approve *without* user interaction, i.e. rules | only interactive approve |
| `statusMessage` on hook config | shown in the agent's UI while a hook runs | unused |
| `type: "mcp_tool"` hooks | call an MCP tool instead of a command | unused |
| Project-level discovery (`<repo>/.codex/hooks.json`) | per-repo hooks | user-level only |
| Async output surfacing | async hook output lands at next safe point | we discard all output on async paths |

## Proposals

Ranked by value over effort. S/M/L sizing as in roadmap-next.md.

### H1. Handle `Interrupt`: stop lying after Esc (S) - **build this first**

Today an interrupted agent keeps its `working` row until the spool record ages
out, because `Interrupt` hits the catch-all `exit 0`. That is a wrong status on
screen for up to a minute, in the one situation where the user *knows* the agent
stopped - they stopped it. It erodes trust in every other row.

Fix is one case branch: `Interrupt) status=idlewait` (the agent is alive and
wants input - the user cut it off mid-answer) plus a `detail=interrupted` so the
row says why. Register the event in both installers. No new plumbing; the pipe
and spool paths already carry it.

> Yes

**Built** as planned, plus `block=idle` so the row says *why* it wants input
rather than only that it does. Registered in both installers. Verified live: an
`Interrupt` event now reports `status=idlewait detail=interrupted block=idle`.

### H2. In-flight tool timing via `tool_use_id` (S/M)

`PreToolUse` and `PostToolUse` share a `tool_use_id`. Write a tiny per-pane file
at Pre (`ts + tool_use_id + detail`), delete it at Post. The plugin already
polls the spool directory; a record that is present *and old* renders as
`Bash cargo test - 94s`, which answers the fleet question the panel exists for:
"is this agent progressing or wedged?" A long-running tool is today
indistinguishable from a hung one.

Cost: one extra file write on the hot path, same pattern as the spool (redirect
plus `mv`, no subprocess). Honours `ZJ_AGENT_HEARTBEAT=0` like the events it
rides on.

> Yes

**Built**, with one design correction. The plan had the panel render the age of
a stamp that was still present; the plugin has no wall clock, so it cannot age a
host epoch. The hook computes the elapsed seconds itself at `PostToolUse` and
sends `tool_secs`, and anything at or over `ZJ_AGENT_SLOW_TOOL` (10s) is appended
to the detail line as `Bash cargo test (94s)`. Clearing is matched on
`tool_use_id`, so a nested inner call cannot clear the outer call's stamp.

### H3. Queue a follow-up from the panel, delivered at `Stop` (M)

`Stop` hooks may return `decision: "block"` with prompt text, which makes the
agent continue with that text as its instruction. That is a *write* channel into
the agent that we already have the read side for: the panel knows the moment a
turn ends.

Flow, mirroring the existing verdict-file pattern
([how-it-works.md](how-it-works.md#answering-permission-prompts)):

1. Panel key (e.g. <kbd>m</kbd> for "message") prompts for a line, writes it to
   `$TMPDIR/zj-agent-mob/followup.<session>.<pane_id>` via `run_command`.
2. On `Stop`, the hook checks for that file. Present: consume it, emit
   `{"decision":"block","reason":"<text>"}`, and report `status=working` with
   `detail=followup: <text>` instead of `done`.
3. Absent: today's behaviour, byte for byte.

Opt-in (`ZJ_AGENT_FOLLOWUP=1`) for the same reason `ZJ_AGENT_APPROVE` is: it is
a path where the panel changes what the agent does, not just what we display.
This subsumes roadmap-next P5 ("reply without leaving the panel") with far less
machinery than driving the pane's stdin, and it works cross-session because the
file, not a pipe, is the transport.

> Yes, no need to opt-in, enable by default, same for ZJ_AGENT_APPROVE (make this the new default)

**Built** default-on, with `ZJ_AGENT_FOLLOWUP=0` and `ZJ_AGENT_APPROVE=0` as the
opt-outs. Two deviations worth recording:

- **The key is <kbd>f</kbd>, not <kbd>m</kbd>.** `m` already opens the free-text
  reply editor. Reusing it would have made one key mean two different transports
  depending on whether the agent happened to be blocked.
- **`Stop` had to become `async: false`.** An async hook's output cannot
  influence its turn, so the `decision: "block"` would have been parsed and
  discarded. `UserPromptSubmit` needed the same change for H5.

Making approval default-on removed a test asserting the opposite
(`approval_is_off_by_default`). It was replaced rather than deleted, by a pair
asserting the new contract and that opting out still works - the safety the old
test protected now rests on the timeout fallback, which is itself tested.

### H4. Rule-based permission verdicts (M)

`ZJ_AGENT_APPROVE=1` answers prompts interactively. The contract also allows a
hook to *decide without asking*. Add a rules file
(`~/.config/zj-agent-mob/approve.rules`, one `allow tool_name [arg-prefix]` per
line) consulted before piping `agent-ask`. Matching rule: emit the verdict
immediately, zero wait, and report the row as `working` rather than `waiting`.

The panel writes rules the same way it writes verdicts today: on an interactive
approve, offer <kbd>A</kbd> as "allow and always allow this tool". This is the
difference between babysitting five agents and only being asked things that are
actually new. Deny rules deliberately excluded from v1: a wrong auto-deny wedges
a turn in a way a wrong auto-allow does not surface worse than the agent's own
sandbox would.

> Yes

**Built** as planned: `allow <tool> [arg-prefix]` per line, allow-only, consulted
before the ask is piped. <kbd>A</kbd> approves and appends the rule. The panel
only ever appends, and only a line the user pressed a key for.

### H5. Fleet awareness via `additionalContext` on `UserPromptSubmit` (M)

The plugin is the only party that can see *all* agents. When two rows share a
`cwd`, the hook can inject one line at the next turn opening: "note: another
agent (pane 4) is working in this repo on: <task>". The agent can then avoid
stepping on a rebase in progress, or at least stop being surprised by moving
files.

Transport: the spool already holds every agent's `cwd` and task; the hook reads
its own directory siblings (no new state), and only on `UserPromptSubmit`, never
on tool events. Strictly informational, capped to one line, opt-in
(`ZJ_AGENT_CONTEXT=1`) because it spends the agent's tokens.

> Yes, make this default, no opt-in

**Built** default-on, opt out with `ZJ_AGENT_CONTEXT=0`. Capped at three peers
rather than one line, and restricted to peers in an *active* state - a finished
agent is not competition for the working tree. An agent is never told about
itself, and a fleet larger than the cap says how many it left out rather than
naming a count it never printed.

### H6. Show the model per row (S)

The payload carries `model` on every event; forward it like `perm_mode` and
render it dimmed on the detail line. Zero-cost to collect, and it answers a real
fleet question ("which of these is the expensive one?") now that mixed-model
fleets are normal. Suppress the common case like `perm_mode=default` is
suppressed: only show a model that differs from the fleet's majority.

> Yes

**Built**, without the majority-suppression. It would make a row's contents
depend on what *other* rows say, so the same agent renders differently as
unrelated agents come and go - the row stops being readable on its own terms.
Instead the id is shortened to the part that actually distinguishes agents
(`claude-sonnet-4-5-20250929` renders `sonnet-4-5`) and an id that does not parse
is truncated rather than dropped, since an unrecognized model is exactly when the
full name is worth seeing.

### H7. `statusMessage` + async output hygiene (S)

Set `"statusMessage": "zj-agent-mob"` on the `PermissionRequest` hook config so
the seconds the sync hook spends polling are attributed on screen instead of
looking like a hang. Audit that all other hooks stay `async: true` on agents
that honour it, and re-test Codex async support each release: the day it works,
the `PermissionRequest` wait stops being on the critical path there too.

> Yes

**Built**: all three synchronous hooks carry a `statusMessage`. The async audit
found the count had grown from one synchronous hook to three, which H3 and H5
required - that is a real cost increase, and the [cost per turn](how-it-works.md#cost-per-turn)
table now states it.

## What not to build

- **`updatedInput` rewriting.** Silently mutating an agent's tool calls from a
  status panel is a category error: the value of this plugin is that it never
  changes what agents do unless the user pressed a key that says so. H3 and H4
  are the sanctioned write paths; both are still driven by a keypress, even
  though both now default to enabled.
- **`UserPromptSubmit` blocking.** Vetoing the user's own prompt from a monitor
  is hostile even when well-meant. `additionalContext` (H5) achieves the useful
  part.
- **`mcp_tool` hooks as transport.** `zellij pipe` is already zero-config and
  session-scoped; an MCP hop adds a server dependency for no reach we lack.
- **Project-level `hooks.json`.** Per-repo hook installs would fork the install
  state the panel reports on ([how-it-works.md](how-it-works.md#the-install-screen))
  and monitoring is a per-user concern, not a per-repo one.

## Sequencing

1. **H1** - it is a bug wearing a feature's clothes; ship in the next release.
2. **H6, H7** - trivial riders on files already being edited for H1.
3. **H2** - biggest display-only value; validates the per-pane sidecar file
   pattern that H3 then reuses.
4. **H4** then **H3** - both extend the verdict-file machinery; approve-rules
   first because it removes interruptions, follow-up second because it adds a
   new kind of control.
5. **H5** last - highest concept risk (spends agent tokens), and it benefits
   from H2's sidecar files being proven.

## Build notes

What only showed up in the building, kept because the reasoning is not
recoverable from the diff.

### macOS `/bin/sh` cannot parse a `case` inside a command substitution

The fleet-note peer scan was first written as a `for` loop inside `$( )` with a
`case` in its body. `dash`, `ksh` and `bash` all accept it; macOS `/bin/sh`
(bash 3.2 in POSIX mode) rejects it with a syntax error on the `;;`. The hook
ships `#!/bin/sh`, and on macOS that *is* this shell, so the loop was lifted into
a top-level `peer_records()` function where 3.2 parses it correctly.

Worth knowing generally: `bash -n script.sh` passing is not evidence the hook
parses, because `bash` and `bash --posix` differ here. The check that matters is
`/bin/sh -n` on macOS specifically.

### The plugin has no wall clock, and this constrains every new field

`spool_age` exists because the plugin cannot read host time. Any new field
carrying a duration must therefore be *computed in the hook* and sent as an
elapsed value - which is what H2 does. An epoch sent to the plugin is unusable
by construction, not merely inconvenient.

### Live verification found what the unit tests could not

Running the hook inside a real Zellij session surfaced a defect the e2e suite
missed: `jq -Rs` drops the trailing newline, so the peer list ran straight into
the advice line (`rebasing the branchCoordinate before ...`). The tests asserted
the note *contained* each part, which was true of the broken output too. A
regression test now asserts the separator specifically.

A second run, against the merged code, found the same class of defect again:
the note printed the real peer total but capped the list at three, so a fleet
of five read as four names it never showed. The tests asserted each part was
present, which the inconsistent version also satisfied.

The general lesson: assertions built from `contains` pass on concatenated and
on internally inconsistent output alike. Where the shape or the arithmetic of
the text matters, assert that specifically - and read the real thing at least
once.

### An unrelated render bug, fixed alongside

The panel's fixed chrome (header, two rules, hint ribbon) was four rows whatever
the pane could hold, so in a pane shorter than that the last line printed was
clipped by the terminal - usually the hint ribbon, sometimes an agent's detail
line. The existing clipping test only covered panes of six rows and up.

The rules are now dropped first in a short pane (they separate, where the header
and the hints carry information), and both the ribbon and the jump counter
refuse to print past the last row. Covered by a new test at 1-5 rows.
