The list now says where each agent runs and what its subagents are up to.

## Worktree identity

The identity column was a fixed 10 characters, so `zj-agent-mob` rendered as `zj-agent-…`. It now grows to fit the longest name on screen, up to 24.

The hook also reports git identity. An agent in a linked worktree shows `repo/worktree` instead of a bare directory name, and grouping by project (<kbd>s</kbd>) groups by repo, so worktrees of one repo sit under one heading. A branch that doesn't match its worktree directory shows up on the detail line as `branch:feat/x`. Identity is derived once per cwd and cached; tool events never fork `git`.

## Pane titles

Rename a pane and that name becomes the row's label. Zellij's defaults (the command name, `Pane #N`) don't count, and the hook's own task summary moves down to the detail line instead of being lost.

## Subagents

Both Claude Code and Codex send `SubagentStart`/`SubagentStop`; the panel now shows them:

- `⑂3` on the main row while subagents run, `⑂✓` once the last one finishes. The badge survives panes too narrow for the detail line.
- The detail line names them: `⑂ 2 running (Explore 1m, Plan 12s), 1 done`.
- <kbd>o</kbd> expands the selected row into one line per subagent, with state and elapsed time.
- Finished subagents stay listed until the next turn starts.

Fuzzy find (<kbd>/</kbd>) also matches the worktree and repo names now.

## Upgrading

```sh
curl -fsSL https://github.com/mohseenrm/zj-agent-mob/releases/download/v0.11.0/init.sh | sh
```

Reload the plugin, since Zellij caches compiled builds:

```sh
zellij action launch-or-focus-plugin --skip-plugin-cache --floating \
  "file:$HOME/.config/zellij/plugins/zj-agent-mob.wasm"
```

The hook changed in this release, so restart your agents after installing. Rows keep working against an older hook; they just won't show git identity or subagent detail until it restarts.

**Full changelog:** https://github.com/mohseenrm/zj-agent-mob/compare/v0.10.3...v0.11.0
