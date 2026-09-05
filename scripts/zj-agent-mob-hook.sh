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
#   ZJ_AGENT_APPROVE     0 disables answering permission prompts from the panel
#   ZJ_AGENT_APPROVE_TIMEOUT  seconds to wait for a verdict (default 30)
#   ZJ_AGENT_APPROVE_RULES    rules file for auto-answered prompts
#   ZJ_AGENT_FOLLOWUP    0 disables delivering a queued follow-up at Stop
#   ZJ_AGENT_CONTEXT     0 disables the fleet note injected at turn start
#   ZJ_AGENT_SPOOL       0 disables the cross-session status spool
#   ZJ_AGENT_SPOOL_DIR   override spool location
#   ZJ_AGENT_FANOUT      0 disables piping urgent transitions to other sessions

# SC2154: event, session_id, cwd, transcript and tool_name are all assigned by
# the `eval` of jq's @sh output below, which shellcheck cannot follow.
# shellcheck disable=SC2154

# Only monitor agents running inside a zellij pane. This is what scopes the
# plugin to the current session: no pane id means nothing to report.
[ -n "$ZELLIJ_PANE_ID" ] || exit 0

# Zellij only ever sets a bare integer here, so anything else is a broken
# environment or a planted one. It reaches both a file path and the args
# string, and the plugin discards a non-numeric pane anyway.
case "$ZELLIJ_PANE_ID" in
  *[!0-9]*) exit 0 ;;
esac

command -v jq >/dev/null 2>&1 || exit 0
command -v zellij >/dev/null 2>&1 || exit 0

PLUGIN="${ZJ_AGENT_PLUGIN:-file:$HOME/.config/zellij/plugins/zj-agent-mob.wasm}"
TOOL="${ZJ_AGENT_TOOL:-claude}"
# Pane ids are only unique within a session, so identity is (session, pane).
# LC_ALL=C and the explicit class keep this byte-wise and locale-independent;
# src/agent.rs::sanitize_session must fold identically.
#
# Sanitizing is lossy, so distinct sessions can fold to one key: "my session"
# and "my_session" both give my_session, and their records would then share a
# spool file and overwrite each other. A name the fold altered therefore carries
# a suffix of its own bytes in hex, which restores the distinction. Names that
# survive the fold unchanged - which is nearly all of them - are left alone, so
# the common filename stays readable and existing records keep their name.
#
# Hex of the raw bytes rather than a checksum, so neither side has to implement
# the other's algorithm: src/agent.rs formats the same bytes the same way.
session_hex() {
  # od is POSIX and does this in one process. Falling back rather than requiring
  # it, because a key that silently loses its suffix would collide again AND
  # disagree with the plugin, which is worse than being slow.
  if command -v od >/dev/null 2>&1; then
    LC_ALL=C printf '%s' "$1" | od -An -tx1 -v 2>/dev/null | LC_ALL=C tr -d ' \n' | LC_ALL=C cut -c1-16
    return
  fi
  _rest=$1
  _out=''
  _i=0
  while [ -n "$_rest" ] && [ "$_i" -lt 8 ]; do
    _ch=$(LC_ALL=C printf '%s' "$_rest" | LC_ALL=C cut -c1)
    _out="$_out$(LC_ALL=C printf '%02x' "'$_ch")"
    _rest=$(LC_ALL=C printf '%s' "$_rest" | LC_ALL=C cut -c2-)
    _i=$((_i + 1))
  done
  printf '%s' "$_out"
}

SESSION=$(printf '%s' "${ZELLIJ_SESSION_NAME:-}" | LC_ALL=C tr -c 'a-zA-Z0-9._-' '_')
if [ -n "${ZELLIJ_SESSION_NAME:-}" ] && [ "$SESSION" != "$ZELLIJ_SESSION_NAME" ]; then
  SESSION="$SESSION-$(session_hex "$ZELLIJ_SESSION_NAME")"
fi

spool_dir() {
  if [ -n "${ZJ_AGENT_SPOOL_DIR:-}" ]; then
    printf '%s' "$ZJ_AGENT_SPOOL_DIR"
  else
    printf '%s/zj-agent-mob-%s/status' "${TMPDIR:-/tmp}" "$(id -u 2>/dev/null || echo 0)"
  fi
}

peer_records() {
  _self=$1
  _cwd=$2
  for _rec in "$(spool_dir)"/*.*; do
    [ -f "$_rec" ] || continue
    _name=${_rec##*/}
    case "$_name" in
      panel.*|inflight.*|*.tmp) continue ;;
    esac
    [ "$_name" = "$_self" ] && continue
    _line=$(head -n 1 "$_rec" 2>/dev/null) || continue
    _fields=$(printf '%s' "$_line" | tr ',' '\n')
    _rcwd=$(printf '%s\n' "$_fields" | sed -n 's/^cwd=//p' | head -1)
    [ "$_rcwd" = "$_cwd" ] || continue
    _rstatus=$(printf '%s\n' "$_fields" | sed -n 's/^status=//p' | head -1)
    case "$_rstatus" in
      working|waiting|idlewait|compact) ;;
      *) continue ;;
    esac
    _rpane=$(printf '%s\n' "$_fields" | sed -n 's/^pane_id=//p' | head -1)
    _rtask=$(printf '%s\n' "$_fields" | sed -n 's/^task=//p' | head -1)
    printf 'pane %s (%s): %s\n' "$_rpane" "$_rstatus" "${_rtask:-no summary}"
  done
}

json=$(cat)
[ -n "$json" ] || exit 0

# @sh quoting keeps values safe to eval even with spaces/quotes in cwd or tool names.
#
# tool_arg is the most identifying argument of a tool call: the file for an
# edit, the command for a shell, the pattern for a search. Codex nests the same
# shape under .tool_input, so one expression serves both agents.
eval "$(printf '%s' "$json" | jq -r '
  @sh "event=\(.hook_event_name // "")
       session_id=\(.session_id // "")
       cwd=\(.cwd // "")
       transcript=\(.transcript_path // "")
       tool_name=\(.tool_name // "")
       tool_arg=\(.tool_input.file_path // .tool_input.command
                  // .tool_input.pattern // .tool_input.path
                  // .tool_input.url // .tool_input.description // "")
       last_msg=\(.last_assistant_message // "")
       notif=\(.message // "")
       notif_type=\(.notification_type // "")
       perm_mode=\(.permission_mode // "")
       agent_type=\(.agent_type // "")
       err_type=\(.error_type // "")
       err_msg=\(.error_message // "")
       model=\(.model // "")
       tool_use_id=\(.tool_use_id // "")
       compact_trigger=\(.trigger // "")"' 2>/dev/null)"

[ -n "$event" ] || exit 0

# Events that fire per-subagent would otherwise overwrite the parent pane's row:
# both share one $ZELLIJ_PANE_ID. They report a delta instead, below.
case "$event" in
  SessionStart)      status=idle ;;
  UserPromptSubmit)  status=working ;;
  Notification)
    # idle_prompt means abandoned, not blocked; only a real prompt is `waiting`.
    case "$notif_type" in
      idle_prompt|agent_needs_input) status=idlewait ;;
      *)                             status=waiting ;;
    esac ;;
  PermissionRequest) status=waiting ;;
  PreToolUse|PostToolUse)
    [ "${ZJ_AGENT_HEARTBEAT:-1}" = "0" ] && exit 0
    status=working ;;
  PostToolUseFailure)
    [ "${ZJ_AGENT_HEARTBEAT:-1}" = "0" ] && exit 0
    status=working ;;
  Stop)              status='done' ;;
  StopFailure)       status=failed ;;
  Interrupt)         status=idlewait ;;
  PreCompact)        status=compact ;;
  PostCompact)       status=working ;;
  SubagentStart|SubagentStop|TaskCreated|TaskCompleted)
    [ "${ZJ_AGENT_HEARTBEAT:-1}" = "0" ] && exit 0
    status='' ;;
  SessionEnd)        status=ended ;;
  *) exit 0 ;;
esac

# Task summary: only re-extract on turn boundaries. Transcripts reach tens of MB,
# so tool events (which fire constantly) deliberately send an empty task and the
# plugin treats empty as "leave unchanged".
task=''
# Stop usually hands us the turn's closing message directly, and that is cheaper
# than any transcript read. It is not always present, so an empty result falls
# through to the transcript below rather than reporting no task at all.
[ "$event" = Stop ] && task=$(printf '%s' "$last_msg" | head -1)
case "$event" in
  Stop|SessionStart|UserPromptSubmit)
    if [ -n "$task" ]; then
      : # already have one from last_assistant_message
    elif [ "$TOOL" = claude ] && [ -n "$transcript" ] && [ -f "$transcript" ]; then
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

tool_secs=''
if [ -n "$tool_use_id" ] && [ "${ZJ_AGENT_HEARTBEAT:-1}" != "0" ]; then
  tdir=$(spool_dir)
  tfile="$tdir/inflight.$SESSION.$ZELLIJ_PANE_ID"
  case "$event" in
    PreToolUse)
      [ -d "$tdir" ] || { mkdir -p "$tdir" 2>/dev/null && chmod 700 "$tdir" 2>/dev/null; }
      if [ -d "$tdir" ]; then
        if printf '%s %s\n' "$(date +%s)" "$tool_use_id" > "$tfile.$$.tmp" 2>/dev/null; then
          mv -f "$tfile.$$.tmp" "$tfile" 2>/dev/null || rm -f "$tfile.$$.tmp" 2>/dev/null || true
        else
          rm -f "$tfile.$$.tmp" 2>/dev/null || true
        fi
      fi ;;
    PostToolUse|PostToolUseFailure)
      prev_inflight=$(head -n 1 "$tfile" 2>/dev/null || true)
      case "$prev_inflight" in
        *" $tool_use_id")
          started=${prev_inflight%% *}
          case "$started" in
            ''|*[!0-9]*) ;;
            *) tool_secs=$(( $(date +%s) - started ))
               [ "$tool_secs" -ge 0 ] 2>/dev/null || tool_secs='' ;;
          esac
          rm -f "$tfile" 2>/dev/null || true ;;
      esac ;;
  esac
fi

# The detail line: the most specific thing we can say about this instant.
case "$event" in
  Notification)
    detail=$notif ;;
  StopFailure)
    detail=${err_msg:-$err_type} ;;
  PreCompact)
    detail="compacting context (${compact_trigger:-auto})" ;;
  Interrupt)
    detail='interrupted' ;;
  PermissionRequest)
    detail="needs approval: ${tool_arg:-$tool_name}" ;;
  PreToolUse|PostToolUse|PostToolUseFailure)
    detail=$tool_name
    [ -n "$tool_arg" ] && detail="$tool_name $tool_arg"
    [ "$event" = PostToolUseFailure ] && detail="$detail (failed)"
    if [ -n "$tool_secs" ] && [ "$tool_secs" -ge "${ZJ_AGENT_SLOW_TOOL:-10}" ] 2>/dev/null; then
      detail="$detail (${tool_secs}s)"
    fi ;;
  *)
    detail='' ;;
esac

# Why the agent is blocked, when it is. `detail` says what the prompt is about;
# this says what kind of answer it wants, which is what decides whether you can
# settle it from the panel or have to read the pane.
block=''
case "$event" in
  PermissionRequest)
    case "$tool_name" in
      ExitPlanMode|exit_plan_mode) block=plan ;;
      *)                           block=tool ;;
    esac ;;
  Notification)
    case "$notif_type" in
      idle_prompt|agent_needs_input) block=idle ;;
      permission_prompt)             block=tool ;;
      *)                             block=question ;;
    esac ;;
  Interrupt) block=idle ;;
esac

followup=''
if [ "$event" = Stop ] && [ "${ZJ_AGENT_FOLLOWUP:-1}" != "0" ]; then
  ffile="${TMPDIR:-/tmp}/zj-agent-mob/followup.$SESSION.$ZELLIJ_PANE_ID"
  if [ -s "$ffile" ]; then
    followup=$(head -n 1 "$ffile" 2>/dev/null || true)
    rm -f "$ffile" 2>/dev/null || true
    if [ -n "$followup" ]; then
      status=working
      detail="followup: $followup"
    fi
  fi
fi

task=$(sanitize "$task")
detail=$(sanitize "$detail")

# Counter deltas rather than absolute values: the hook is stateless, and each
# event only knows that one more thing started or finished.
subagent_delta=0
task_delta=0
task_done_delta=0
case "$event" in
  SubagentStart)  subagent_delta=1 ;;
  SubagentStop)   subagent_delta=-1 ;;
  TaskCreated)    task_delta=1 ;;
  TaskCompleted)  task_done_delta=1 ;;
esac

# `default` is the common case and would be noise on every row.
[ "$perm_mode" = default ] && perm_mode=''
agent_type=$(sanitize "$agent_type")
perm_mode=$(sanitize "$perm_mode")
model=$(sanitize "$model")

if [ "${ZJ_AGENT_DEBUG:-0}" = "1" ]; then
  mkdir -p "$HOME/.cache/zj-agent-mob"
  printf '%s pane=%s tool=%s event=%s status=%s task=[%s] detail=[%s]\n' \
    "$(date +%H:%M:%S)" "$ZELLIJ_PANE_ID" "$TOOL" "$event" "$status" "$task" "$detail" \
    >> "$HOME/.cache/zj-agent-mob/hook.log"
fi

ARGS="pane_id=$ZELLIJ_PANE_ID,session=$SESSION,tool=$TOOL,status=$status,session_id=$session_id,cwd=$cwd,task=$task,detail=$detail,block=$block,perm_mode=$perm_mode,model=$model,agent_type=$agent_type,tool_secs=$tool_secs,subagent_delta=$subagent_delta,task_delta=$task_delta,task_done_delta=$task_done_delta"

zellij pipe --name agent-status --plugin "$PLUGIN" --args "$ARGS" >/dev/null 2>&1 || true

# The pipe above reaches only this session's plugin. A panel in another session
# otherwise waits for its next poll to notice, which is too slow for the states
# that actually need you. Fan those out directly, and only those: this costs one
# subprocess per target, so the high-frequency heartbeats never take this path.
case "$status" in
  waiting|idlewait|failed|done)
    if [ "${ZJ_AGENT_FANOUT:-1}" != "0" ] && [ -n "$SESSION" ]; then
      for beacon in "$(spool_dir)"/panel.*; do
        [ -f "$beacon" ] || continue
        # The filename is the sanitized name, which is what compares against
        # our own $SESSION. The contents are the name zellij knows it by:
        # sanitizing is lossy, so "my session" is keyed my_session and only the
        # stored spelling can actually be addressed.
        key=${beacon##*/panel.}
        [ -n "$key" ] || continue
        [ "$key" = "$SESSION" ] && continue
        target=$(head -n 1 "$beacon" 2>/dev/null)
        [ -n "$target" ] || target=$key
        zellij --session "$target" pipe --name agent-status \
          --plugin "$PLUGIN" --args "$ARGS" >/dev/null 2>&1 || true
      done
    fi ;;
esac

# Cross-session transport. The pipe above only reaches this session's plugin, so
# a panel elsewhere gets status from this file instead. Deliberately not a
# subprocess and never blocking: this runs on the critical path of every turn.
if [ "${ZJ_AGENT_SPOOL:-1}" != "0" ] && [ -n "$SESSION" ] && [ "$status" != ended ]; then
  sdir=$(spool_dir)
  # Records hold task summaries, which are the user's own prompts, so on a
  # shared /tmp the directory must not be world-readable. Only set at creation:
  # re-chmodding every event costs a syscall on the hot path for nothing.
  [ -d "$sdir" ] || { mkdir -p "$sdir" 2>/dev/null && chmod 700 "$sdir" 2>/dev/null; }
  if [ -d "$sdir" ]; then
    sfile="$sdir/$SESSION.$ZELLIJ_PANE_ID"
    # A record is a whole snapshot, not a delta, so it cannot lean on the
    # plugin's "empty means unchanged" rule the way the pipe does. Events that
    # carry no session_id or cwd (Notification, the counter events) inherit the
    # last known values instead of blanking them - session_id in particular is
    # what stops a recycled pane id inheriting a dead agent's status.
    # Counter events carry no status; an empty one is unparseable and would
    # strand the row at `unknown`, so inherit it too.
    if [ -z "$session_id" ] || [ -z "$cwd" ] || [ -z "$status" ] || [ -z "$model" ]; then
      prev=$(head -n 1 "$sfile" 2>/dev/null || true)
      if [ -n "$prev" ]; then
        [ -n "$session_id" ] || session_id=$(printf '%s' "$prev" | tr ',' '\n' | sed -n 's/^session_id=//p' | head -1)
        [ -n "$cwd" ] || cwd=$(printf '%s' "$prev" | tr ',' '\n' | sed -n 's/^cwd=//p' | head -1)
        [ -n "$status" ] || status=$(printf '%s' "$prev" | tr ',' '\n' | sed -n 's/^status=//p' | head -1)
        [ -n "$model" ] || model=$(printf '%s' "$prev" | tr ',' '\n' | sed -n 's/^model=//p' | head -1)
      fi
    fi
    if [ -z "$status" ]; then
      SKIP_SPOOL=1
    fi
    SPOOL_ARGS="pane_id=$ZELLIJ_PANE_ID,session=$SESSION,tool=$TOOL,status=$status,session_id=$session_id,cwd=$cwd,task=$task,detail=$detail,block=$block,perm_mode=$perm_mode,model=$model,agent_type=$agent_type"
    # Rename is atomic within a filesystem, so a reader sees the old record or
    # the new one, never a half-written one. $$ keeps concurrent hooks apart.
    if [ "${SKIP_SPOOL:-0}" != "1" ] && printf 'ts=%s,%s\n' "$(date +%s)" "$SPOOL_ARGS" > "$sfile.$$.tmp" 2>/dev/null; then
      mv -f "$sfile.$$.tmp" "$sfile" 2>/dev/null || rm -f "$sfile.$$.tmp" 2>/dev/null || true
    else
      rm -f "$sfile.$$.tmp" 2>/dev/null || true
    fi
  fi
fi

# SessionEnd retires the agent, so its record must not outlive it and colour a
# recycled pane id later.
if [ "$status" = ended ] && [ -n "$SESSION" ]; then
  rm -f "$(spool_dir)/$SESSION.$ZELLIJ_PANE_ID" 2>/dev/null || true
fi

# Answering a permission prompt from the panel.
#
# The plugin cannot write to stdin of an already-running process, so the verdict
# travels through a file it drops via `run_command`. Timing out falls through to
# the agent's own prompt, which is why every failure here is silent: the worst
# case must be the normal interactive experience, never a wedged turn.
if [ "$event" = PermissionRequest ] && [ "${ZJ_AGENT_APPROVE:-1}" = "1" ]; then
  rules_file="${ZJ_AGENT_APPROVE_RULES:-$HOME/.config/zj-agent-mob/approve.rules}"
  if [ -n "$tool_name" ] && [ -r "$rules_file" ]; then
    while IFS= read -r rule || [ -n "$rule" ]; do
      case "$rule" in
        ''|'#'*) continue ;;
      esac
      rule_verb=${rule%% *}
      [ "$rule_verb" = allow ] || continue
      rule_rest=${rule#allow }
      rule_tool=${rule_rest%% *}
      [ "$rule_tool" = "$tool_name" ] || continue
      case "$rule_rest" in
        "$rule_tool")
          printf '{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow"}}}\n'
          exit 0 ;;
      esac
      rule_prefix=${rule_rest#"$rule_tool" }
      case "$tool_arg" in
        "$rule_prefix"*)
          printf '{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow"}}}\n'
          exit 0 ;;
      esac
    done < "$rules_file"
  fi

  vdir="${TMPDIR:-/tmp}/zj-agent-mob"
  vfile="$vdir/verdict.$SESSION.$ZELLIJ_PANE_ID"
  mkdir -p "$vdir" 2>/dev/null || true
  rm -f "$vfile" 2>/dev/null || true

  # The timeout travels with the prompt: when it passes, the hook has already
  # fallen through to the agent's own prompt and nothing is reading the verdict
  # file any more. Without it the panel would keep offering `a` for a prompt
  # that can no longer be answered, and report success for a verdict that was
  # written into the void.
  approve_timeout=${ZJ_AGENT_APPROVE_TIMEOUT:-30}
  zellij pipe --name agent-ask --plugin "$PLUGIN" \
    --args "pane_id=$ZELLIJ_PANE_ID,session=$SESSION,verdict_file=$vfile,tool_name=$tool_name,tool_arg=$tool_arg,timeout=$approve_timeout" \
    >/dev/null 2>&1 || true

  # Poll rather than block on a FIFO: a FIFO open() with no reader hangs past
  # any timeout, and Codex has no working async hooks to absorb that.
  waited=0
  limit=$approve_timeout
  while [ "$waited" -lt "$limit" ]; do
    if [ -s "$vfile" ]; then
      verdict=$(cat "$vfile" 2>/dev/null)
      rm -f "$vfile" 2>/dev/null || true
      case "$verdict" in
        allow)
          printf '{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow"}}}\n'
          exit 0 ;;
        deny)
          printf '{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"deny"}}}\n'
          exit 0 ;;
      esac
      break
    fi
    sleep 1
    waited=$((waited + 1))
  done
  # No verdict: say nothing and let the agent prompt as usual.
  rm -f "$vfile" 2>/dev/null || true
fi

if [ -n "$followup" ]; then
  printf '%s' "$followup" | jq -Rs '{decision:"block", reason:.}' 2>/dev/null || true
  exit 0
fi

if [ "$event" = UserPromptSubmit ] && [ "${ZJ_AGENT_CONTEXT:-1}" != "0" ] && [ -n "$cwd" ]; then
  peers=$(peer_records "$SESSION.$ZELLIJ_PANE_ID" "$cwd")
  if [ -n "$peers" ]; then
    count=$(printf '%s\n' "$peers" | grep -c . 2>/dev/null || echo 0)
    shown=$count
    [ "$shown" -gt 3 ] 2>/dev/null && shown=3
    more=$((count - shown))
    tail_note=''
    [ "$more" -gt 0 ] && tail_note=" (and $more more)"
    printf '%s\n' "$peers" | head -3 | jq -Rs --arg n "$count" --arg t "$tail_note" \
      '{hookSpecificOutput:{hookEventName:"UserPromptSubmit",
        additionalContext:("zj-agent-mob: " + $n + " other agent(s) are working in this same directory right now:\n" + . + "Coordinate before wide-reaching changes (rebases, file moves, dependency bumps)." + $t)}}' \
      2>/dev/null || true
  fi
fi

exit 0
