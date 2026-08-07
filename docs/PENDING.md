## Pending tasks

- [ ] System notifications when an agent needs you — investigated and specced in
      [notifications.md](notifications.md), not implemented. Recommendation is `osascript`
      via `run_command` with per-agent debouncing and burst coalescing, defaulting to
      `waiting,failed`. Open: SSH sessions notify on the wrong machine.

Recently completed:

- [x] Full-feature demo recording ([demo.md](demo.md)): `demo/tour.gif`, linked from the README,
      rendered by `./scripts/demo/render.sh`. Driven by `zellij action` from outside the session
      rather than keystrokes, so it is reproducible. Still open: regenerating `docs/img/*.png`
      under the current theme, a real-agent clip, and a `dump-screen` CI assertion job.
- [x] Richer hook data, F1-F9 from [roadmap-ux.md](roadmap-ux.md): tool arguments on the
      detail line, real notification text, the turn's closing message, a `failed` state from
      `StopFailure`, compaction, a permission-mode badge, subagent fan-out counts, native task
      progress, and opt-in approve/reject from the panel. F10 (model badge) skipped: Claude
      sends no model field, so the column would be blank for half the rows.
- [x] Quick actions to install hooks in Claude Code and Codex — in-panel install
      screen (`i`), backed by per-target `init.sh` subcommands
- [x] GitHub workflow for building and releasing artifacts (`.github/workflows/release.yml`)
- [x] GitHub workflow for PRs (`.github/workflows/ci.yml`)
- [x] Updated and polished readme, styled and syntax-highlighted
- [x] PR template (`.github/pull_request_template.md`)
- [x] Instructions on how to install with zellij (keybinding + layout, both validated)
- [x] Local dev setup
- [x] Troubleshooting
- [x] End-to-end tests for the Claude Code and Codex install path
      (`tests/e2e-install.sh`), run in CI

- [ ] Self-contained install script for agent harnesses
    - [ ] Claude Code
    - [ ] Codex
