# System notifications when an agent needs you

Status: **investigation complete, nothing implemented.** Findings measured on macOS 15 /
Darwin 25.3.0, Zellij 0.44.3, WezTerm, on 2026-08-07.

The panel already knows the moment an agent starts waiting. It just cannot tell you unless you
are looking at it. `popup_on_waiting` un-hides the floating pane, which works only if Zellij is
the focused window - the case where you least need telling. This doc records what was measured
and what each option costs. The recommendation is
[`osascript` via `run_command`](#4-option-b-run_command-to-a-platform-notifier-recommended)
with per-agent debouncing.

- [1. The gap](#1-the-gap)
- [2. What the plugin can actually call](#2-what-the-plugin-can-actually-call)
- [3. Measurements](#3-measurements)
- [4. Option B: `run_command` to a platform notifier](#4-option-b-run_command-to-a-platform-notifier-recommended)
- [5. Options considered and rejected](#5-options-considered-and-rejected)
- [6. UX sketch](#6-ux-sketch)
- [7. Implementation notes](#7-implementation-notes)
- [8. Open questions](#8-open-questions)

## 1. The gap

`handle_status` already computes exactly the signal a notification needs
([`src/state.rs`](../src/state.rs)):

```rust
newly_waiting = changed && status == Status::Waiting;
...
if newly_waiting && self.popup_on_waiting && self.hidden {
    self.hidden = false;
    host::show_self(true);
}
```

`newly_waiting` is edge-triggered - it fires on the transition, not on every heartbeat - which is
the hard part of notification design already solved. What follows it is the whole problem:
`show_self` raises a floating pane inside Zellij. If your attention is on a browser, a different
terminal, or another desktop, nothing reaches you. The agent waits until you happen to look.

That is the actual complaint. An agent that has been blocked for eleven minutes on a
one-keystroke approval is the most expensive failure mode this panel has, and it is invisible
precisely when you have context-switched away - the only time it matters.

Four states are worth surfacing, and they are not equally urgent:

| Status | Why notify | Frequency |
|---|---|---|
| `waiting` | Needs a decision now; blocks all progress | Low, bursty |
| `failed` | Agent is stopped (rate limit, billing, auth) | Low |
| `idle-wait` | Has been blocked on you for a while | Low |
| `done` | Long run finished | **High** - the noise risk |

## 2. What the plugin can actually call

`zellij-tile` 0.44.3 has **no notification API**. Grepping the shim for `notify`, `alert`,
`bell`, `attention`, `urgent`, and `badge` returns nothing. The panel's only routes to the
outside world are:

| Mechanism | Available | Note |
|---|:--:|---|
| `run_command(argv, ctx)` | yes | Already used by the install screen |
| `web_request(...)` | yes | Needs `WebAccess` permission |
| `print!` to the pane | yes | Consumed by Zellij's renderer, not the terminal |
| Native notification call | **no** | Does not exist in the API |

`run_command` is the only one already granted (`PermissionType::RunCommands`, requested in
`load()` for the install screen), so it adds no new permission prompt.

**How Zellij runs it matters.** `run_command` is dispatched to the Zellij *server* process,
which spawns the command as its own child. The server is started from the user's GUI login
session, so a spawned `osascript` inherits a normal desktop context - the same path
`install.sh status` already takes successfully on every panel load. Results come back
asynchronously as `RunCommandResult`, tagged with a context key.

## 3. Measurements

All run on this machine, in a live Zellij session.

**Notifier availability:**

| Binary | Present | Note |
|---|:--:|---|
| `osascript` | **yes** | `/usr/bin/osascript`, built into macOS |
| `terminal-notifier` | no | Homebrew, not installed |
| `notify-send` | no | Linux only |
| `alerter` | no | Not installed |

**`osascript` latency** (`display notification`):

```
run 1: 145ms       (cold)
run 2: 108ms
burst of 5: 482ms total -> 96ms each
```

Confirmed visible on screen. Sound (`sound name "Ping"`) works. Escaped quotes inside the
message body work. Dispatching from a detached/`setsid` parent works, so a fire-and-forget
notification does not need the plugin to wait on it.

96ms is cheap enough to call inline, but a **burst of five stacks five separate banners** in
Notification Center. That is the finding that shapes the design more than latency does.

**Terminal escape sequences (OSC 9, OSC 777):** written from inside a Zellij pane, both came
back as literal text rather than being consumed by the terminal - Zellij's renderer intercepts
the sequence rather than passing it through. A direct `/dev/tty` write to bypass shell capture
could not be tested from this tool context ("Device not configured"), so passthrough is
**unverified rather than disproven**. It remains an open question, but the literal-text result
makes OSC an unpromising primary route under a multiplexer.

## 4. Option B: `run_command` to a platform notifier (recommended)

Notify from the plugin, not the hook, and gate it on the transition the plugin already computes.

```rust
// state.rs, replacing the current popup block
if newly_waiting {
    self.notify(pane_id, Status::Waiting);
}
```

**Why the plugin rather than the hook.** The hook is per-agent and stateless - it cannot know
that three other agents just went `waiting`, so it cannot debounce or coalesce. The plugin holds
every agent's state and already computes edge transitions. Putting the logic in the hook would
mean five processes each independently deciding to notify.

**Command, by platform:**

```sh
# macOS - always present, no install step
osascript -e 'display notification "MESSAGE" with title "zj-agent-mob"'

# macOS, better app identity + click-to-focus, if the user has it
terminal-notifier -title 'zj-agent-mob' -message 'MESSAGE' -group zj-agent-mob

# Linux
notify-send -a zj-agent-mob 'zj-agent-mob' 'MESSAGE'
```

Detect once at load via `run_command` and cache which is available, rather than probing per
notification.

**Argv, not a shell string.** The message contains a task summary and a tool argument, both
attacker-influenced in the sense that they come from arbitrary repo content. Pass them as
separate argv elements exactly as
[`write_verdict`](../src/host.rs) already does for the verdict path:

```rust
run_command(&["osascript", "-e", &script], ctx);   // script built with the message as a bound arg
```

Never interpolate a task summary into a string that a shell parses.

**Debouncing is the design, not a detail.** Three rules, all necessary:

1. **Per-agent cooldown.** One notification per pane per N seconds (default 60). An agent that
   flips `waiting -> working -> waiting` while you are mid-approval must not fire twice.
2. **Coalesce a burst.** If more than one agent transitions inside a short window (~2s), send
   one summary - `3 agents need input` - not three banners. This is the measured five-stacked-
   banners problem.
3. **Suppress when focused.** If the Zellij pane is already visible and focused, the panel is
   doing its job; a system notification is redundant. `popup_on_waiting` covers that case.

**Configuration**, following the existing `popup_on_waiting` pattern in `load()`:

| Key | Default | Meaning |
|---|---|---|
| `notify` | `waiting,failed` | Comma-separated statuses; empty disables |
| `notify_cooldown` | `60` | Per-agent seconds between notifications |
| `notify_sound` | `false` | Play a sound with the notification |
| `notify_command` | auto | Override the detected notifier |

Defaulting to `waiting,failed` and **not** `done` is deliberate: `done` is the high-frequency
trigger and the one most likely to make the whole feature annoying enough to switch off. It is
available for those who want it, off by default.

**Cost:** ~80 lines in `state.rs` plus a `host::notify` shim. No new permission. No new
dependency. Degrades to today's behaviour when no notifier is found.

## 5. Options considered and rejected

**Option A: terminal escape sequence (OSC 9 / OSC 777).** Zero dependencies and would work over
SSH, which is genuinely attractive. Rejected as the *primary* route because the sequences came
back as literal text from inside a Zellij pane, and because support is per-terminal - WezTerm,
kitty, and foot handle OSC 777 differently, and Terminal.app not at all. Worth revisiting as an
opt-in fallback for remote sessions if `run_command` proves insufficient; see
[open questions](#8-open-questions).

**Option C: notify from the hook script.** Simplest possible change - one `osascript` line in
`zj-agent-mob-hook.sh`. Rejected because the hook is stateless and per-agent: it cannot debounce
across agents, cannot suppress when the panel is focused, and would need the notifier detection
duplicated per invocation. It also puts a 100ms GUI call on the agent's critical path, which is
precisely what the async-hook design exists to avoid.

**Option D: `web_request` to a notification service.** Pushover, ntfy.sh, or similar. Would
reach your phone, which nothing else here does. Rejected for the default path: needs the
`WebAccess` permission, an account, a network round trip, and sends task summaries off the
machine. Reasonable as a future opt-in for long unattended runs, not as the built-in behaviour.

**Option E: Zellij's own bell / attention API.** Does not exist in `zellij-tile` 0.44.3.
Re-check on future Zellij releases; if it lands it would be strictly better than shelling out.

## 6. UX sketch

The notification itself:

```
┌────────────────────────────────────────────┐
│  zj-agent-mob                              │
│  codex · web needs approval                │
│  rm -rf node_modules                       │
└────────────────────────────────────────────┘
```

Coalesced, when several land at once:

```
┌────────────────────────────────────────────┐
│  zj-agent-mob                              │
│  3 agents need input                       │
│  web, api, dotfiles                        │
└────────────────────────────────────────────┘
```

A failure:

```
┌────────────────────────────────────────────┐
│  zj-agent-mob                              │
│  claude · api stopped                      │
│  rate limited, retry in 30s                │
└────────────────────────────────────────────┘
```

In-panel, notifications need no new surface. The only visible change is an indicator on rows
that have already been announced, so you can tell what you were told about:

```
▶ 1 ● codex   waiting    2s  web        Fix flaky checkout test
      └ needs approval: rm -rf node_modules · notified · pane:5
```

The install screen gains a row, since "are notifications going to work" is exactly the kind of
thing that needs checking without leaving the panel:

```
zj-agent-mob   install

▶ c  Claude Code hooks    ✓ installed
  x  Codex hooks          ✓ installed
  p  Plugin wasm          ✓ installed
  n  Notifications        ✓ osascript
```

`✗ no notifier found` when neither `osascript` nor `notify-send` exists, which on Linux is the
prompt to install `libnotify`.

## 7. Implementation notes

**Where the state lives.** `Agent` gains one field:

```rust
/// When this agent last produced a system notification. Debouncing is
/// per-agent: one noisy agent must not suppress a different one's alert.
pub(crate) last_notified: f64,
```

`State` gains the detected notifier and the pending-coalesce buffer.

**The clock.** `self.now` only advances on `Timer` events, and `arm_timer` runs the timer only
while an agent `is_active()`. A cooldown measured against `self.now` therefore **stops advancing
once every agent is idle or waiting** - exactly the state a waiting agent is in. This is the
subtle bug to avoid: either arm the timer whenever a cooldown is outstanding, or measure the
cooldown against a monotonic source rather than `now`. Worth an explicit test.

**Testing.** `host::notify` no-ops off-wasm like every other host call, so the debounce and
coalesce logic is unit-testable with no Zellij and no GUI:

- a `waiting` transition notifies
- a heartbeat that does not change status does not
- a second transition inside the cooldown does not
- the same transition after the cooldown does
- N agents transitioning together produce one coalesced notification, not N
- `notify=` (empty) disables entirely
- a status not in the configured list does not fire
- the cooldown still expires when no agent is active (the clock bug above)

Follow the existing pattern: assert on what `host::notify` *would* have been called with, via a
recording stub, rather than on a real notification.

**Verification beyond unit tests.** The measurements in §3 came from driving `osascript`
directly. The one thing that could not be verified from a non-TTY tool context is the plugin's
own `run_command` reaching the notifier - a shim on `install.sh` confirmed the script runs and
`osascript` exits 0 when invoked directly, but the floating plugin pane could not be driven to
trigger it. Since the install screen already round-trips `run_command` on every panel load, the
mechanism is proven; the notification-specific path still wants one manual check in a real
session before this ships.

## 8. Open questions

- **OSC passthrough.** Unverified rather than disproven. If Zellij does pass OSC 777 to the host
  terminal, it becomes a genuinely better fallback for SSH sessions, where `run_command` fires
  the notification on the *wrong machine*. Needs a `/dev/tty` test from a real terminal.
- **SSH is the real hole.** `run_command` notifies on the host running Zellij. Attaching to a
  remote session from a laptop means the notification appears on the server. Detecting
  `SSH_CONNECTION` and degrading to OSC (or nothing) is unsolved.
- **Click-to-focus.** `terminal-notifier` supports an activation action that could focus the
  waiting pane. `osascript` does not. Worth it only if enough people install
  `terminal-notifier`.
- **Should `idle-wait` notify at all?** It is already a "you have been ignoring this" signal.
  Notifying on it may just be a second reminder for something you decided to ignore. Off by
  default in the proposal above; may deserve to be unavailable.
- **Interaction with `popup_on_waiting`.** Both fire on the same transition. The proposal
  suppresses the notification when Zellij is focused, but "is Zellij focused" is not directly
  knowable from the plugin - the closest proxy is `self.hidden`, which tracks whether the panel
  is hidden, not whether the window has OS focus.
