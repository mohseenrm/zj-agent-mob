## Pending tasks

All clear. Recently completed:

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
