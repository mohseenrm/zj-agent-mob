# Demo recording

How the README demo is produced, and what it cost to get there.

> [!NOTE]
> **Shipped:** a single full-feature tour at [`demo/tour.gif`](../demo/tour.gif), linked from the
> README. Render it with `./scripts/demo/render.sh` (~2 min).
>
> ```
> scripts/demo/
>   lib.sh           # zellij-action helpers: emit, pane_ids, show_panel, wait_for, key,
>                    #   spawn_session, open_agent, mock_in
>   mock-agent.sh    # a stand-in Claude/Codex transcript for the prop panes
>   stage-tour.sh    # the 11-act tour, driven from outside the session
>   tour.tape        # VHS tape: starts Zellij, backgrounds the staging script, records
>   render.sh        # entry point; fails if fewer than 12 act markers are logged
> ```
>
> The rest of this document is the investigation behind those four files. The traps in
> [Why the obvious approach fails](#why-the-obvious-approach-fails) are all load-bearing - each one
> cost a render cycle, and each is re-introducible by an innocent-looking edit.

- [Conclusion first](#conclusion-first)
- [What was verified](#what-was-verified)
- [Why the obvious approach fails](#why-the-obvious-approach-fails)
- [Architecture: drive from outside, record from inside](#architecture-drive-from-outside-record-from-inside)
- [Tool comparison](#tool-comparison)
- [Known VHS issues that shape the design](#known-vhs-issues-that-shape-the-design)
- [What shipped](#what-shipped)
- [File layout](#file-layout)
- [Open questions](#open-questions)

## Conclusion first

**Use VHS for everything scripted, and a screen recording only for the one "real agents" clip.**

VHS was the leaning in the original question and it survives contact: a full Zellij session with
the floating plugin panel renders correctly inside it. That was the open risk, and it is now
closed by a working artifact rather than by reasoning.

The non-obvious part, and the thing this plan is really about: **do not drive the demo with
keystrokes.** Drive it with `zellij action` from outside the session. That single decision is what
makes the demo deterministic, and it is what the first three attempts got wrong.

Two artifacts, different jobs, and they are not interchangeable:

| Artifact | Tool | Agent state | Why |
|---|---|---|---|
| Hero GIF, feature clips, doc stills | VHS + `zellij action` | Synthetic, piped in | Reproducible, diffable, CI-runnable |
| One "this is real" clip | `screencapture -v` | Real `claude` / `codex` | Proves it is not a mockup |

Nothing does both. VHS cannot host real agents deterministically (nondeterministic timing and
output, plus API keys in CI); a screen recording cannot be re-run identically. Ship both, and be
honest in the README about which is which.

## What was verified

Run locally on macOS 26.3.1, `zellij 0.44.3`, `vhs 0.11.0` (installed via `brew install vhs`,
which pulls `ttyd` + `ffmpeg`). These are results, not expectations:

| Claim | Result |
|---|---|
| Zellij renders inside VHS | **Yes.** Box drawing, truecolor, powerline status bar all pixel-clean |
| The floating plugin panel renders | **Yes.** Border, title, `PIN [ ]`, colored status badges |
| Selected-row background highlight | **Renders correctly.** VHS bug [#344](https://github.com/charmbracelet/vhs/issues/344) does not bite this panel |
| Nerd Font glyphs | **Yes**, with `Set FontFamily "JetBrainsMono Nerd Font"`. Without it, glyphs render as `▯` |
| `launch-or-focus-plugin` headless | **Yes**, returns the pane id (`plugin_4`) |
| `zellij action pipe` populating rows | **Yes**, once ordered correctly (see below) |
| `Screenshot` tape command | **Broken.** Exit 2, no PNG written, even in a 6-line tape |
| `Output frames/` as the PNG workaround | **Yes**, 170 PNGs for a 7s tape |
| `dump-screen --path` | **Yes** — plain-text panel capture, useful for assertions |

The end-to-end proof: a tape that creates three panes, pipes three agent statuses, and renders
the panel with `1 waiting · 1 working · 0 done`, per-row detail lines (`tab:1 · pane:1`,
`1 turns · tab:1 · pane:0`), and the keybinding footer.

## Why the obvious approach fails

Worth recording, because each of these cost a full attempt and each will be re-attempted by
anyone who edits the tapes.

**1. `Ctrl+S` does not reliably reach Zellij from a tape.** The natural tape is `Ctrl+S` then `c`,
mirroring the README keybinding. Do not rely on it. Use
`zellij action launch-or-focus-plugin` instead.

This one took several attempts to pin down, and two plausible explanations turned out to be wrong,
so the evidence is worth keeping:

- **Not a missing keybinding.** The first failure used a throwaway `ZELLIJ_CONFIG_DIR` with no
  `keybinds`, so `c` was genuinely unbound; a stray keystroke leaked into the pane and opened
  Neovim (`E492: Not an editor command`). Using the real stowed config at
  [`~/dotfiles/.config/zellij/config.kdl`](file:///Users/mohseen.mukaddam@opendoor.com/dotfiles/.config/zellij/config.kdl),
  which binds `Ctrl s` → Session mode and `session { bind "c" }` → this plugin, the chord **did**
  work once. So the binding is correct.
- **Not shell flow control.** `Ctrl+S` is XOFF, so `stty -ixon` was the obvious fix. Verified: it
  does **not** help.
- **What it actually is:** reproduced in isolation - real config, nothing else running, no staging
  script - `Ctrl+S` leaves Zellij in Normal mode, with the status bar still advertising
  `Ctrl s SESSION`. The keypress reaches Zellij (the top-right hint row changes) but does not
  select Session mode. This is consistent with the known xterm.js/ttyd legacy-keymap gaps behind
  [#728](https://github.com/charmbracelet/vhs/issues/728); `Ctrl+S` is simply not delivered as
  Zellij expects. It worked exactly once, which makes it timing-dependent - the worst property for
  a recording that must render identically every time.

`zellij action launch-or-focus-plugin` is unconditional and needs no mode at all. The cost is that
the recording no longer *shows* the user's keystroke, so state the binding in surrounding prose or
overlay it as a caption.

Note also that `Ctrl+s` must be written `Ctrl+S` in a tape - lowercase is a parse error
(`Expected control character with args, got s`).

**2. Fake `pane_id`s get culled.** Piping `pane_id=1,2,3` into a fresh session shows nothing.
[`state.rs:270`](../src/state.rs) `reconcile()` drops any agent whose pane is absent from the
`PaneManifest`, and correctly so. The demo must create **real** terminal panes and pipe their
**real** ids. Get them from `zellij action list-panes`, matching `^terminal_` only — a naive
`grep -o '[0-9]*'` also matches `plugin_0` and silently shifts every id.

**3. `zellij action pipe` hangs with no payload.** With no `<PAYLOAD>` argument it listens on
STDIN, so from a script it blocks forever. Fix with `</dev/null` (verified: exit 0 immediately).

**4. Order matters: pipe before launching the panel.** Piping after
`launch-or-focus-plugin` raced the plugin's load and produced an empty
`no agents in this session` panel. Because `pipe --plugin` auto-launches the plugin anyway, send
the first status *first*, then focus it. This is also more honest: it is the order that happens in
real use, where a hook fires before you ever open the panel.

**5. The user's shell profile leaks into frames.** Default `Set Shell "zsh"` ran the full profile,
so the recording opened with fastfetch output and a `work.zsh: no such file` error. Force a
sterile shell: `Set Shell "bash"`, `PS1='$ '`, and `zellij options --default-shell bash` for the
inner panes. `Set Shell "sh"` is rejected (`invalid shell sh`).

**6. Zellij's startup tips cover the panel.** A first-run Zellij shows an "About Zellij / Zellij
Tip #2" modal squarely where the demo goes - verified overlapping the panel. A single `Escape` in
the tape dismisses it, which is the approach to use when recording against the real config.
Alternatively a throwaway `Env ZELLIJ_CONFIG_DIR` with `show_startup_tips false` and
`show_release_notes false` avoids it entirely, at the cost of losing the real keybinds.

**7. Focus must be restored before any keystroke.** The staging script ends with `new-pane` calls,
so focus sits on the last-created pane, and subsequent tape keystrokes go there rather than to the
panel. End every staging script with
`zellij action focus-pane-id terminal_<first-id>`. Even so, prefer
`launch-or-focus-plugin` over keystrokes per trap #1.

**8. `set -e` plus `zellij pipe` kills the run silently.** The worst bug of the build, because it
looked exactly like a hang. `zellij action pipe` can exit nonzero *even when the message was
delivered*. Under `set -e` that aborted the staging script mid-act, leaving a panel that still
ticked its elapsed timers - so 45 of the 52 recorded seconds were a frozen list, with an empty
stderr and no clue why. Every `emit`/`send-keys`/`focus` helper ends in `|| true`.

The lesson generalises: **instrument the staging script with act markers.** `render.sh` counts them
and fails when fewer than 8 appear, so a truncated tour can never be silently published again.

**9. Prop panes must not run a shell.** `new-pane` returned `terminal_1/2/3` successfully, but
`list-panes` showed them gone a second later, so all but one agent row was culled. The new panes
inherited `$SHELL`, whose profile failed and exited. They now run `sh -c 'sleep 100000'`: the pane
only has to *exist* for `reconcile()`, and nothing reads what is inside it. They are also renamed,
because `sh -c sleep 100000` as a pane title gives the game away.

**10. A plugin pane is only dumpable while focused.** `dump-screen --pane-id plugin_N` writes zero
bytes, and so does dumping while a terminal pane holds focus. `wait_for` therefore focuses the
panel before each poll. This is why `key()` re-focuses on every call: a key sent while a prop pane
has focus is swallowed by that pane, which is the other way the tour froze.

**11. View toggles reset the floating geometry.** Pressing `i` for the install screen snapped the
floating pane back to its default size, putting the prop panes back in frame. `key()` re-asserts
the geometry after every keystroke via `fill_frame`.

**12. A different `--configuration` is a different plugin.** Zellij routes both launches and
pipes by (url, configuration). The tour passes `discover=false` to suppress the process scan
(otherwise the panel also lists the real agents on the recording machine). Passing it to
`launch-or-focus-plugin` but not to `pipe` produced **two** Agent Mob panes: one holding every
row, one empty. `lib.sh` keeps a single `PLUGIN_CONF` used by all three call sites, and
`stage-tour.sh` asserts at the end that no session has more than one panel.

**13. `--stacked` and `-d` are mutually exclusive.** `zellij action new-pane -d down --stacked`
exits with `The argument '--direction <DIRECTION>' cannot be used with '--stacked'` - and because
every helper ended in `|| true`, four pane creations failed in total silence. Instrumenting the
helpers to log `new-pane`'s output is what found it; that logging is worth restoring the moment
anything looks wrong.

**14. `$0` inside a sourced file is the caller.** `MOCK="$(dirname "$0")/mock-agent.sh"` in
`lib.sh` resolved relative to `stage-tour.sh`, then to the repo root, and every prop pane silently
fell back to a bare shell. `stage-tour.sh` now exports `ZJ_DEMO_DIR` and `lib.sh` reads that.

**15. A nested session is not "live" to the plugin.** `spawn_session` creates the extra sessions
by running `zellij attach --create` inside a pane. Those sessions appear in `zellij list-sessions`
but **not** in the plugin's `SessionUpdate`, which only reports sessions with a directly-attached
client - so their rows render `unknown · (session exited)`. Verified three ways:
`attach --create-background` (no client at all) is worse, and a `ttyd` host never starts without a
browser connecting. This is a real property of Zellij, not a demo artifact: a user whose sessions
are attached in their own terminals sees live status. The demo shows the `unknown` state honestly
and still demonstrates the jump, which works either way.

**16. Two things that look like session state are not.** After act 9 the recorded client is in
another session, so (a) `PANEL_PANE` is stale - it names a pane in the session just left - and
(b) `zellij list-sessions | grep (current)` reports the session the *staging script* runs from,
which never moved. Use `za_in <session> list-clients` to find where the client actually landed.

**17. The startup-tip modal is created lazily.** Hiding it in `spawn_session` is too early: it
appears when a client first attaches, which is the jump itself. Hide it *after* the jump, in every
candidate session.

### Decision: record against the real stowed config

Recording uses the real `~/.config/zellij` (stowed from `~/dotfiles`) rather than a hermetic
throwaway config. The tradeoffs were reviewed and accepted:

| Tradeoff | Consequence, accepted |
|---|---|
| `theme "brutus"` | Artifacts show the personal theme, not Zellij's default |
| Personal profile | fastfetch output, real hostname, and work paths can appear in frames; `Set Shell "bash"` + `PS1='$ '` mitigates the worst of it |
| Startup tips | Not disabled in the real config; dismiss with `Escape` (trap #6) |
| Not reproducible off this machine | Tapes will not render in CI, so **the Phase 5 CI recording job is out of scope.** The `dump-screen` assertion job stays viable, since it needs no VHS or fonts |

This supersedes open questions 1 and 2 below. Worth knowing what it costs: the demo becomes a
local, one-machine build step rather than a CI artifact.

## Architecture: drive from outside, record from inside

The load-bearing idea. VHS records a terminal running Zellij; a **separate staging script** drives
that session over Zellij's CLI control surface. The tape itself types almost nothing.

```
┌─ VHS (ttyd + xterm.js in headless Chromium → ffmpeg) ──────────┐
│                                                                │
│   $ zellij -s demo            ← the only thing the tape types  │
│   ┌──────────────────────────────────────────────────────┐     │
│   │ pane 0    pane 1    pane 2    ┌── Agent Mob ──────┐  │     │
│   │ (real terminal panes, so      │ ▶ 1 codex waiting │  │     │
│   │  reconcile() keeps the rows)  │   2 claude working│  │     │
│   │                               └───────────────────┘  │     │
│   └──────────────────────────────────────────────────────┘     │
└────────────────────────────▲───────────────────────────────────┘
                             │  zellij -s demo action ...
                  ┌──────────┴───────────┐
                  │  scripts/demo/*.sh   │  new-pane, list-panes,
                  │  (backgrounded)      │  pipe, launch-or-focus-plugin,
                  └──────────────────────┘  send-keys, dump-screen
```

The tape reduces to: set up env, background the staging script, start Zellij, `Show`, sleep.
Everything interesting is a shell script that can be tested on its own without recording anything.

Why this is worth the indirection:

- **Deterministic.** `zellij action` either succeeds or returns nonzero. Keystrokes are timing-dependent.
- **Debuggable.** Run the staging script against a live session and watch it; no 3-minute render loop.
- **Assertable.** `dump-screen --path` gives text to grep, so CI can verify content, not just that a file was produced.
- **Real keys where they matter.** `j`/`k`/`x`/`i` still go through `zellij action send-keys`, so the recording genuinely exercises the plugin's key handling rather than faking it.

The available surface is broad: `new-pane`, `list-panes`, `focus-pane-id`, `new-tab`, `go-to-tab`,
`pipe`, `launch-or-focus-plugin`, `send-keys`, `write-chars`, `toggle-floating-panes`,
`dump-screen`, `resize`, `rename-pane`.

## Tool comparison

| Tool | Fidelity | Deterministic / CI | Output | Real agents? | Verdict |
|---|---|---|---|---|---|
| **VHS** | High | **Yes** — tape is source of truth | gif, mp4, webm, `frames/` | No | **Primary.** Verified working |
| `screencapture -v` | Pixel-perfect | No | `.mov` | **Yes** | **Secondary**, for one real clip |
| asciinema + `agg` | Excellent, Nerd Font fallback bundled | Poor (captures a human) | `.cast` → gif | Yes | Skip — VHS covers it, `.cast` in a README needs a player |
| asciinema + `svg-term-cli` | Vector, smallest | Poor | svg | Yes | Optional nicety |
| terminalizer | Decent | Poor | gif | Yes | Skip, least maintained |

On the options in the original question: asciinema's real advantage is capturing an authentic
human session, but its output is a `.cast` needing a player, and GitHub READMEs want a GIF or MP4.
`agg` closes that gap but reintroduces the size problem. terminalizer is strictly dominated. The
one thing VHS genuinely cannot do — real agents — is better served by `screencapture -v`, which is
already on every Mac and needs no extra dependency.

Note for the macOS path: grant screen-recording permission to the terminal **before** scripting
it. `ffmpeg -f avfoundation -list_devices true -i ""` blocks indefinitely on the permission
prompt rather than failing.

## Known VHS issues that shape the design

| Issue | Impact | Mitigation |
|---|---|---|
| `Screenshot` broken in 0.11.0 (verified) | Cannot grab stills the documented way | `Output frames/`, pick a frame with ffmpeg |
| [#344](https://github.com/charmbracelet/vhs/issues/344) whitespace background | Would hit row highlights | **Verified not affecting this panel.** Re-check after VHS upgrades |
| [#728](https://github.com/charmbracelet/vhs/issues/728) `Ctrl+<digit>` unsupported | `1`-`9` quick-jump not sendable as a chord | Plain `Type "3"` is fine; only `Ctrl`+digit is broken |
| No mouse support | Mouse interaction unrecordable | Keyboard only. Not a real loss; the plugin is keyboard-driven |
| `Set Width/Height` are **pixels** | Cannot request 120x34 directly | Tune font size + pixels, verify the grid via `dump-screen` |
| Fixed terminal size per tape | No mid-tape resize | One tape per size if a narrow-layout demo is wanted |

`Hide`/`Show` only pause frame capture (`PauseRecording`/`ResumeRecording`); the PTY keeps
running. That is what makes the "hide the messy setup" pattern safe.

## What shipped

A single tour rather than the originally-planned hero clip plus six feature clips. One recording
that covers everything is less to keep in sync, and the README only has one place a reader looks
first. The seven acts:

| Act | Shows |
|---|---|
| 0 setup | The panel on first open |
| 1 appear | Agents arriving mid-turn, tool call on the detail line, `[bypassPermissions]` badge |
| 2 fan-out | Subagent counters and native task progress (`2 subagents: explore · 4/7 tasks`) |
| 3 waiting | `waiting` sorting to the top on its own, then the approve/reject box, approved with `a` |
| 4 states | `compact`, `failed` with the reason, `done` with the turn's closing message, `idle` |
| 5 nav | `j`/`k` moving the selection |
| 6 kill | `x` arming the row, with the red "press x again" confirm, then backing out |
| 7 install | The install screen and back |
| 8 cross-session | Agents in two *other* Zellij sessions, each with its own panes |
| 9 hop | `Enter` on a foreign row switching sessions and landing on that agent's pane |
| 10 back | The same panel from inside the session jumped to |

The panes behind the panel run [`mock-agent.sh`](../scripts/demo/mock-agent.sh) rather than a
bare `sleep`, so the moment act 9 jumps into one it shows a plausible transcript. With
`ZJ_MOCK_PROMPT` set it ends on a permission prompt, which is what a `waiting` row means.

Not done, and deliberately left:

- **Regenerated `docs/img/*.png` stills.** The six existing screenshots predate the current theme,
  so replacing them is all-or-nothing (open question 2).
- **The real-agent clip.** Still the most convincing possible artifact, and still manual
  (open question 3).
- **The `dump-screen` CI assertion job.** The staging scripts already exercise
  pipe → reconcile → render, so this is mostly wiring: run `stage-tour.sh` against a headless
  session with no VHS and assert on the dumped text. Cheapest remaining win.

## File layout

```
scripts/demo/
  lib.sh                 # za(), emit(), pane_ids(), show_panel(), wait_for(), key()
  stage-tour.sh          # the 7 acts, with `act` markers render.sh checks
  tour.tape              # the only tape
  render.sh              # entry point
docs/
  demo.md                # this file
  img/                   # hand-taken stills, not yet regenerated
demo/
  tour.gif               # committed artifact, 45s, ~580K
```

Generated GIFs live in `demo/`, not `docs/img/`, so the reproducible artifact and the hand-taken
stills stay easy to tell apart.

On size: 580K for 45s at 1500x900 is acceptable for a README, but it is the single largest file in
the repo. If more clips get added, switch them to MP4 and keep GIF only for the one the README
autoplays. `gifsicle --lossy=80 -k 128 -O2` is the lever if it needs to shrink without re-recording.

## Open questions

Questions 1 and 2 (theme, hermetic vs. real config) are **settled**: record against the real
stowed config, accepting the personal theme. What remains:

1. **Size.** 1500x900 at `FontSize 15` gave a comfortable grid. Smaller renders sharper on GitHub
   at the cost of visible rows.
2. **Do the stills get regenerated now, or left alone?** Regenerating is strictly better for
   consistency but churns six binary files in the diff. Note the current `docs/img/*.png` were
   taken under a different theme, so mixing new and old artifacts will look inconsistent - it is
   close to all-or-nothing.
3. **Is the real-agent clip worth the manual step?** It is the most convincing artifact and the
   only unrepeatable one.
4. **Does the fastfetch-in-panes noise need suppressing?** Accepted in principle, but the panes
   behind the panel are visibly full of system-info output in the test renders. A one-line
   `stage-*.sh` tweak (`clear` in each pane, or `--default-shell bash`) would tidy the frames
   without giving up the real config.
