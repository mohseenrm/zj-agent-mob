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

Transcripts reach tens of megabytes, so summaries are re-read only on turn-opening boundaries (`SessionStart`, `UserPromptSubmit`) from a bounded `tail -n 300` window. Tool events send an empty `task=`, and the plugin treats empty as "leave unchanged".

`Stop` needs no transcript read at all: both agents put the turn's closing text in `last_assistant_message`, so a finished row shows what actually happened rather than the summary from before the turn.

## Counter events

`SubagentStart` / `SubagentStop` and `TaskCreated` / `TaskCompleted` all fire on the parent's pane, so they cannot carry a status without overwriting the pane's own state. They send an empty `status=` plus a delta (`subagent_delta=1`, `task_done_delta=1`) instead, and the plugin applies it to the existing row. Deltas rather than absolute counts, because the hook is stateless: each event only knows that one more thing started or finished. Counters reset when a new turn begins, and saturate at zero so a stray `Stop` cannot underflow.

These events are high-frequency during a fan-out, so they honour `ZJ_AGENT_HEARTBEAT=0` alongside the tool events.

## Answering permission prompts

Opt-in via `ZJ_AGENT_APPROVE=1`, and the only hook that deliberately blocks a turn. `PermissionRequest` is registered with `async: false` for exactly this reason; every other Claude hook stays async so it can never stall the agent.

The plugin cannot write to the stdin of an already-running hook, so the verdict travels through a file:

1. The hook pipes `agent-ask` with a `verdict_file` path and blocks, polling for it.
2. <kbd>a</kbd> / <kbd>r</kbd> in the panel writes `allow` or `deny` to that path via `run_command`.
3. The hook reads it and prints `{"hookSpecificOutput":{"decision":{"behavior":"allow"}}}`.

Polling rather than a FIFO: opening a FIFO with no reader blocks past any timeout, and Codex parses `async` but does not implement it, so there is nothing to absorb a hang. On timeout the hook prints nothing and exits 0, which falls through to the agent's own prompt - the worst case is the normal interactive experience, never a wedged turn.

The path is passed to `sh -c` as a positional argument rather than interpolated into the command string, so nothing outside the plugin is ever parsed by a shell.

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
