Rows from other sessions used to read `unknown` far more often than they should have, and `gone` when the session was demonstrably running. Both labels threw away something the panel already knew. A row now keeps the last status it was told and says how old that is:

```
   2 ⠋ claude  working    1m18s  api  [bypassPermi…]
     └ last seen 1m03s ago · Bash cargo test --release
```

Dimmed, spinner stopped, elapsed still counting. The status is a minute old, and the row says so rather than pretending it knows nothing.

## What was going wrong

Three separate things, all of which looked like the same bug from the outside.

**A quiet minute read as unknown.** Hooks fire around tool calls. An agent thinking, or inside one long `cargo build`, sends nothing, and after 60 seconds the row was overwritten with `unknown` and its elapsed reset to `0s`. The turn's duration was lost, and the row flapped back the moment the tool returned.

**Rows could get stuck at `gone` forever.** Urgent transitions are pushed straight to other panels, and that pipe creates the row. The panel then asked Zellij which sessions were live -- except `SessionUpdate` only ever names the session its own server owns, so the answer was always "just this one" and the new row was pronounced dead. Dead rows stop polling. By the time a process scan proved the session alive, the status had already been replaced, and the record on disk couldn't undo it. The row sat at `unknown` while the file next to it said `done`.

Attaching to a session to look at its panel is exactly what triggered this.

**Fresh panels ignored what was already on disk.** Open a panel after an agent finished and it showed `found` -- "there's a process here, no idea what it's doing" -- while a `done` record from ten minutes ago sat unread. Old records were dropped rather than shown as old.

## What changed

A foreign row keeps its status until something newer arrives. Nothing else may overwrite it -- not the clock, not the session list.

- Past 60 seconds a row is marked **stale**: same label, same elapsed, dimmed, spinner frozen, `last seen 2m ago` leading the detail line.
- Session liveness comes from the process scan alone, which is the only source that can actually see another session's server. Until a scan has reported, nothing is declared dead.
- A row whose session really did exit reads `gone`, sorts to the bottom, leaves the header counts, and keeps what it was doing: `(session exited · was done)`.
- Old records are applied and marked stale, instead of being thrown away.
- Turns are now counted for agents in other sessions too.
- `unknown` is gone as a status. Nothing produces it any more.

The panel repaints once a second while its clock is running, so a stale row's elapsed keeps ticking after its spinner stops.

## Upgrading

Press <kbd>U</kbd> in the panel, or:

```sh
curl -fsSL https://github.com/mohseenrm/zj-agent-mob/releases/download/v0.12.1/init.sh | sh
```

The hook script is unchanged from v0.12.0, so running agents don't need restarting. Panels in other sessions keep the old code until they reload.

**Full changelog:** https://github.com/mohseenrm/zj-agent-mob/compare/v0.12.0...v0.12.1
