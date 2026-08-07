# How it works

- [Status transport](#status-transport)
- [Task summaries](#task-summaries)
- [The install screen](#the-install-screen)

## Status transport

Each hook event runs `scripts/zj-agent-mob-hook.sh`, which forwards status to the plugin:

```sh
zellij pipe --name agent-status --plugin file:...wasm \
  --args "pane_id=$ZELLIJ_PANE_ID,tool=claude,status=waiting,..."
```

`$ZELLIJ_PANE_ID` is set by Zellij in every terminal pane and is inherited by processes started there, so it identifies the exact pane. If it isn't set, the hook exits immediately, which is what scopes monitoring to the current session: agents outside Zellij are ignored, and each session's plugin instance only sees its own panes.

`zellij pipe --plugin` auto-launches the plugin if it isn't running, so there's no daemon, socket, or state file.

## Task summaries

- **Claude Code** transcripts contain an `ai-title` record: Claude's own rolling summary of the session (e.g. "Review Zellij plugin UI rendering documentation"). Falls back to `last-prompt`, then the pane title.
- **Codex** has no equivalent, so the first `event_msg` / `user_message` record from the session rollout is used. Raw `response_item` records are skipped because they're padded with `AGENTS.md` and `<INSTRUCTIONS>` preamble.

Transcripts reach tens of megabytes, so summaries are re-read only on turn boundaries (`SessionStart`, `UserPromptSubmit`, `Stop`) from a bounded `tail -n 300` window. Tool events send an empty `task=`, and the plugin treats empty as "leave unchanged".

`/goal` sets a session-scoped hook but leaves no on-disk artifact, so goals aren't readable. `ai-title` tracks the real work anyway. For a manual override:

```sh
zellij pipe --name agent-label --args "pane_id=3,label=whatever you want"
```

## The install screen

The plugin runs in WASI with no access to `$HOME`, so it cannot read `settings.json` to see whether hooks are installed. Instead it shells out via Zellij's `run_command` to `~/.config/zj-agent-mob/install.sh status`, which prints one `key=state` line per target:

```text
claude=installed
codex=absent
plugin=installed
hook=installed
```

Results arrive asynchronously as a `RunCommandResult` event, tagged with a context key so install output is never confused with another command's. After any toggle the plugin re-reads state rather than assuming the change landed, because the installer can succeed partially (hooks written, plugin not built).
