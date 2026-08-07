# Discovering agents that have not reported

Status: investigation complete, nothing implemented. Findings verified on macOS with
Zellij 0.44.3 on 2026-08-07.

The panel only knows about an agent once that agent's hook fires. Reload the plugin and it
starts empty; an idle agent emits no events, so the panel says **"no agents in this session"**
while three agents sit there. The message is indistinguishable from a broken install, which is
the actual complaint - the first instinct is to go debug the hooks, and the hooks are fine.

This doc records what was measured and what the options cost. The recommendation is
[process-environment discovery](#3-option-c-process-environment-scan-recommended).

- [1. The gap](#1-the-gap)
- [2. What the WASI sandbox allows](#2-what-the-wasi-sandbox-allows)
- [3. Option C: process-environment scan](#3-option-c-process-environment-scan-recommended)
- [4. Options considered and rejected](#4-options-considered-and-rejected)
- [5. UX sketch](#5-ux-sketch)
- [6. Implementation notes](#6-implementation-notes)
- [7. Open questions](#7-open-questions)

## 1. The gap

Status reaches the plugin exactly one way: a hook fires, the hook shells out to `zellij pipe`,
`handle_status` upserts a row (`src/state.rs:63`). Nothing else populates `agents`.

Two consequences:

- **The plugin holds no state across a reload.** Zellij keeps the compiled module for the
  session's lifetime, but a new plugin instance starts with `State::default()`.
- **Idle agents are invisible.** Hooks fire on activity. An agent that finished 30 minutes ago
  has nothing left to announce.

A session with three healthy, idle agents therefore renders identically to one with no agents
and a broken install. `docs/troubleshooting.md` walks through five checks for
"no agents in this session" and does not mention this case, which is the most likely one
immediately after a plugin reload.

## 2. What the WASI sandbox allows

Persisting the agent list looked like the obvious fix, so the mounts were measured directly
with a throwaway probe plugin that requested **no permissions** and wrote its results to
`/host`:

| Mount | Write | Read | Survives reload | Survives new session |
|---|:--:|:--:|:--:|:--:|
| `/data` | OK | OK | no | no |
| `/tmp` | OK | OK | **yes** | **yes** |
| `/host` | OK | OK | yes | yes |
| absolute host path | - | **error 44** | - | - |

Three things this settles:

- **Persistence is available.** `/tmp` survived across two entirely separate Zellij sessions,
  and needs no `FullHdAccess`. The assumption in `src/install.rs:3` ("WASI gives the plugin no
  `$HOME` and no filesystem") is right about arbitrary host paths and wrong about the mounts.
- **The plugin's `/tmp` is sandboxed.** Searching `/private/tmp` and `/private/var/folders`
  found nothing the plugin wrote. The docs call it "an arbitrary position in the system's
  temporary filesystem", and it is not reachable from the host. **The hook cannot write a file
  the plugin can read.**
- **`/host` is real but moving.** It maps to the last-focused terminal's cwd, so it is not a
  stable location for state.

Reference: [Zellij plugin filesystem API](https://zellij.dev/documentation/plugin-api-file-system.html).

## 3. Option C: process-environment scan (recommended)

Every agent process carries `ZELLIJ_PANE_ID` and `ZELLIJ_SESSION_NAME` **in its own
environment**, inherited from the pane's pty. This is the key property: it does not depend on
how the agent was started.

Measured against five live agents:

| pid | parent | started as | `ZELLIJ_PANE_ID` | session |
|---|---|---|---|---|
| 45985 | zellij server | command pane | 2 | `zj-agent-mob` |
| 47845 | **`/bin/zsh`** | **typed into a shell** | 3 | `zj-agent-mob` |
| 46438 | zellij server | command pane | 4 | `zj-agent-mob` |
| 67819 | zellij server | command pane | 11 | `code` |
| 3823 | zellij server | command pane | 2 | `fresh-2` |

Row 2 is the one that matters. `pane command="claude"` in the layout only exists for panes
Zellij launched as commands; an agent started by typing `claude` into an existing shell has no
such entry, and `PaneInfo.terminal_command` is `None` for it. The environment variable is
present either way.

The scan, session-scoped. **One `ps` invocation, not one per pid** - `ps axeww` prints every
process's environment in a single call, so the per-pid loop the first draft used is unnecessary:

```sh
ps axeww -o pid=,command= 2>/dev/null | awk -v want="$sess" '
{
  cmd = $2; sub(/.*\//, "", cmd)
  if (cmd != "claude" && cmd != "codex") next
  pane = ""; sess = ""
  for (i = 3; i <= NF; i++) {
    if ($i ~ /^ZELLIJ_PANE_ID=/)      pane = substr($i, 16)
    if ($i ~ /^ZELLIJ_SESSION_NAME=/) sess = substr($i, 21)
  }
  if (pane != "" && sess == want) print pane, cmd, $1
}' | sort -un
```

Verified output (macOS 26.3.1, Zellij 0.44.3):

```
$ scan.sh zj-agent-mob        $ scan.sh code
2 claude 45985                11 claude 67819
3 claude 47845
4 claude 46438
6 codex  43820
```

Correct session scoping - the `code` and `fresh-2` agents stay out. Runtime **70ms** for the
single-call form, against **147ms** for the per-pid loop. The plugin already holds `RunCommands`,
so no new permission and no new prompt on first load.

Note `ps -e eww` and `ps -eo ... eww` both return **no environment at all** on macOS - the POSIX
`-e` and the BSD `e` collide silently rather than erroring. The BSD-style `axeww` is required.
This is an easy bug to ship without noticing, because the command still exits 0 and still prints
a process list; only the env is missing, so the scan just finds nothing.

**Match on the executable basename, not the command line.** `pgrep -f '^claude'` misses the
shell-launched agent, because its `claude` is a child of `zsh` and the pattern anchors against
the wrong process. Matching `comm` catches all five.

### What this is not

A heuristic, and worth being honest about the edges:

- A pane running something incidentally named `claude` shows up as an agent.
- An agent wrapped in a differently-named launcher does not. This is not hypothetical - three of
  the panes measured here were launched via `npm exec`, and they are only detected because the
  final `claude` process replaces the wrapper in the tree. A launcher that stays resident as the
  matched process's parent is fine; one that stays resident *as* the process is not.
- Environment is fixed at exec time and never updated. Whether `ZELLIJ_PANE_ID` still resolves
  after a pane is moved between tabs is **untested** - see open questions.
- It reports *a process exists in this pane*, never what that agent is doing. Discovered rows
  carry no status, task, or cwd until a hook fires.

## 4. Options considered and rejected

### Option A: persist the agent list to `/tmp`

Works mechanically - `/tmp` persistence is real and measured. Rejected on behaviour: restored
rows cannot be proven live. After a reload the panel would show three agents with 30-minute-old
statuses and no way to know whether the panes still exist or what those agents are doing now.
Stale rows that look live are worse than an honest empty panel, which is the current failure and
at least reads as "I know nothing".

### Option B: `PaneInfo.terminal_command`

Already present in every `PaneManifest` the plugin receives and currently ignored by
`reconcile` (`src/state.rs:152`). Zero new machinery, no `run_command`, no permission.

Rejected because it only covers command panes. `terminal_command` is documented as "if this is
a command pane", and an agent started by typing `claude` into a shell has no command pane -
confirmed by pid 47845 above. This was the first recommendation in discussion and it does not
meet the requirement.

**It is worse than "incomplete" - it is actively wrong.** `zellij action dump-layout` exposes the
same underlying data, and on the session used for these measurements it disagrees with reality:

```
ps says (truth)          dump-layout claims
pane 2  claude           pane 2  command="npm"  args "exec" "@growthbook/mcp@latest"
pane 3  claude           pane 3  command="claude"
pane 4  claude           pane 4  command="npm"  args "exec" "@growthbook/mcp@latest"
```

Three live agents, one reported, two mislabelled as `npm`. The recorded command is the pane's
*original* launch command and is never updated, so a pane that started as `npm exec ...` and now
runs `claude` still reports `npm` indefinitely. Pane 2 in that table is the pane this
investigation was conducted from.

So the earlier note that it is a "zero-cost corroborating signal" is **withdrawn**: a signal that
is stale by design cannot corroborate anything. If it agrees with the scan it adds nothing, and
if it disagrees the scan is the one that is right. Use it for the pane *title* if that is useful
to display, never to decide whether an agent is present.

### Option D: docs only

Add the scenario to `troubleshooting.md` as the first thing to check after a plugin reload.
Zero code. Does not fix the misleading screen, only explains it. Worth doing regardless of which
option ships, because the scan is a heuristic and will have its own gaps.

## 5. UX sketch

Discovered-but-silent agents, before any hook has fired:

```
zj-agent-mob   3 idle

  1 ○ claude  idle       --   zj-agent-mob   no report yet
      └ tab:1 · pane:2
  2 ○ claude  idle       --   zj-agent-mob   no report yet
      └ tab:1 · pane:3
  3 ○ claude  idle       --   zj-agent-mob   no report yet
      └ tab:1 · pane:4

 ↵ jump   1-9 quick   x kill   d dismiss   i install   q hide
```

The moment any of them acts, the hook upgrades that row in place - status, task and cwd fill in
and the row stops being a placeholder. Nothing persists, nothing goes stale, and a closed pane
drops out on the next `PaneUpdate` exactly as today.

`--` rather than a zero duration is deliberate: the plugin does not know when a discovered agent
last did anything, and inventing `0s` would claim otherwise.

## 6. Implementation notes

Four pieces:

1. **Session name.** The plugin does not currently know it. There is no `get_session_name`
   shim; subscribe to `EventType::SessionUpdate` and take the `SessionInfo` where
   `is_current_session` is true (`zellij-utils-0.44.3/src/data.rs:1784`). Without this the scan
   would list agents from every session on the machine.
2. **The scan.** `run_command` in the style of `src/install.rs`, parsed from
   `RunCommandResult`. Re-run on a timer and on `PaneUpdate`, not on every tick - 85ms of `ps`
   per frame is not free.
3. **Merge, do not append.** A discovered pane that already has a hook-reported agent must
   update that row, never add a second one. Pane 3 in the table above is both hook-reporting
   *and* discoverable, so without dedup every active agent doubles.
4. **A placeholder status.** Discovered rows need a state distinct from `idle`, which today
   means "reported, then went quiet". Reusing `idle` would erase the difference between "this
   agent told us it finished" and "we found a process and know nothing about it".

Test coverage to add:

- `tests/e2e-install.sh`-style shell coverage for the scan script: session scoping, the
  shell-launched case, and no output when nothing matches.
- Rust tests for merge/dedup: discovered-then-hook, hook-then-discovered, and a discovered pane
  that disappears.

## 7. Open questions

- ~~**Codex detection is unverified.**~~ **Resolved.** Verified against a real Codex process: a
  pane spawned with `zellij action new-pane -- codex` was detected as `6 codex 43820`, carrying
  `ZELLIJ_PANE_ID` and `ZELLIJ_SESSION_NAME` exactly as the Claude processes do. Codex is a
  native binary, not a node wrapper, so `comm` reports a bare `codex` and basename matching needs
  no special case.
- **Linux portability.** `ps axeww` is BSD syntax and everything here was measured on macOS.
  Linux can read `/proc/<pid>/environ` directly, which is cheaper and more reliable. The scan
  probably wants two implementations rather than one portable compromise. Note the macOS form is
  narrower than it looks: see the `-e` versus `axeww` trap in section 3.
- **Pane id after a move.** The env var is frozen at exec time. If moving a pane to another tab
  reassigns its id, a moved agent would be attributed to the wrong pane - or to none. One manual
  check: note a pid's `ZELLIJ_PANE_ID`, move the pane, re-run the scan, compare against
  `PaneManifest`.
- **Scan cadence.** On a timer, on `PaneUpdate`, or only when the agent list is empty? The last
  is cheapest and covers the reported symptom, but leaves an agent started later invisible until
  it acts.
- **Does a discovered agent count in the header?** `3 idle` conflates "known idle" with "found,
  unknown". The counts drive the summary line, so this changes what the header means.
- **Killing a discovered agent.** `x kill` targets a pane id, which discovery provides, so it
  would work. Whether it *should* be offered for a row the panel knows nothing about is a
  separate call.
