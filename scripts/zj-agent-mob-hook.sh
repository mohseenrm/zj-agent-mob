#!/bin/sh
# zj-agent-mob hook: reports Claude Code / Codex agent status to the zellij plugin.
#
# Installed by init.sh into Claude Code's settings.json and Codex's hooks.json.
# Receives the hook event as JSON on stdin. Always exits 0 so it can never block
# or fail an agent turn.
#
# Env:
#   ZJ_AGENT_TOOL        claude | codex   (default: claude)
#   ZJ_AGENT_HEARTBEAT   0 disables PreToolUse/PostToolUse status refresh
#   ZJ_AGENT_PLUGIN      override plugin path
#   ZJ_AGENT_DEBUG       1 logs to ~/.cache/zj-agent-mob/hook.log

# SC2154: event, session_id, cwd, transcript and tool_name are all assigned by
# the `eval` of jq's @sh output below, which shellcheck cannot follow.
# shellcheck disable=SC2154

# Only monitor agents running inside a zellij pane. This is what scopes the
# plugin to the current session: no pane id means nothing to report.
[ -n "$ZELLIJ_PANE_ID" ] || exit 0

command -v jq >/dev/null 2>&1 || exit 0
command -v zellij >/dev/null 2>&1 || exit 0

PLUGIN="${ZJ_AGENT_PLUGIN:-file:$HOME/.config/zellij/plugins/zj-agent-mob.wasm}"
TOOL="${ZJ_AGENT_TOOL:-claude}"

json=$(cat)
[ -n "$json" ] || exit 0

# @sh quoting keeps values safe to eval even with spaces/quotes in cwd or tool names.
eval "$(printf '%s' "$json" | jq -r '
  @sh "event=\(.hook_event_name // "")
       session_id=\(.session_id // "")
       cwd=\(.cwd // "")
       transcript=\(.transcript_path // "")
       tool_name=\(.tool_name // "")"' 2>/dev/null)"

[ -n "$event" ] || exit 0

case "$event" in
  SessionStart)                   status=idle ;;
  UserPromptSubmit)               status=working ;;
  Notification|PermissionRequest) status=waiting ;;
  PreToolUse|PostToolUse)
    [ "${ZJ_AGENT_HEARTBEAT:-1}" = "0" ] && exit 0
    status=working ;;
  Stop)                           status='done' ;;
  SessionEnd)                     status=ended ;;
  *) exit 0 ;;
esac

# Task summary: only re-extract on turn boundaries. Transcripts reach tens of MB,
# so tool events (which fire constantly) deliberately send an empty task and the
# plugin treats empty as "leave unchanged".
task=''
case "$event" in
  SessionStart|UserPromptSubmit|Stop)
    if [ "$TOOL" = claude ] && [ -n "$transcript" ] && [ -f "$transcript" ]; then
      # Must be `tail -n` (lines), NOT `tail -c` (bytes): a byte cut lands mid-line
      # and jq aborts the whole stream on the partial record:
      #   jq: parse error: Invalid numeric literal at line 1, column 9
      tail_buf=$(tail -n 300 "$transcript" 2>/dev/null)
      task=$(printf '%s\n' "$tail_buf" \
        | jq -rc 'select(.type=="ai-title") | .aiTitle' 2>/dev/null | tail -1)
      [ -n "$task" ] || task=$(printf '%s\n' "$tail_buf" \
        | jq -rc 'select(.type=="last-prompt") | .lastPrompt' 2>/dev/null | tail -1)
    elif [ "$TOOL" = codex ] && [ -n "$session_id" ]; then
      codex_home="${CODEX_HOME:-$HOME/.codex}"
      roll=$(find "$codex_home/sessions" -name "*-$session_id.jsonl" -type f 2>/dev/null | head -1)
      if [ -n "$roll" ]; then
        task=$(jq -r 'select(.type=="event_msg") | .payload
          | select(.type=="user_message") | .message' "$roll" 2>/dev/null | head -1)
      fi
    fi
    ;;
esac

# --args is comma-separated key=value, so commas and newlines must go.
sanitize() {
  printf '%s' "$1" | tr '\n\r\t,' '    ' | cut -c1-60 | sed 's/  */ /g; s/^ *//; s/ *$//'
}
task=$(sanitize "$task")
detail=$(sanitize "$tool_name")

if [ "${ZJ_AGENT_DEBUG:-0}" = "1" ]; then
  mkdir -p "$HOME/.cache/zj-agent-mob"
  printf '%s pane=%s tool=%s event=%s status=%s task=[%s]\n' \
    "$(date +%H:%M:%S)" "$ZELLIJ_PANE_ID" "$TOOL" "$event" "$status" "$task" \
    >> "$HOME/.cache/zj-agent-mob/hook.log"
fi

zellij pipe --name agent-status --plugin "$PLUGIN" \
  --args "pane_id=$ZELLIJ_PANE_ID,tool=$TOOL,status=$status,session_id=$session_id,cwd=$cwd,task=$task,detail=$detail" \
  >/dev/null 2>&1 || true

exit 0
