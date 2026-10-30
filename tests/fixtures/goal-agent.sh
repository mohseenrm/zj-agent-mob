#!/bin/sh
set -eu
if [ "${1:-}" = --emit ]; then
  shift
  printf '{"hook_event_name":"Stop","session_id":"%s","cwd":"/tmp/goal-qa-%s","last_assistant_message":"Turn verified. Objective remains open."}' "$2" "$3" |
    sh "$1/scripts/zj-agent-mob-hook.sh"
  exit 0
fi
root=$1
work=$2
thread=$3
layout=$4
export ZJ_AGENT_TOOL=codex
export ZJ_AGENT_PLUGIN="file:$work/zj-agent-mob.wasm"
export ZJ_AGENT_PLUGIN_CONFIG="goal_display=$layout,icons=unicode,check_updates=false,notify="
export ZJ_AGENT_FANOUT=0 ZJ_AGENT_FOLLOWUP=0 ZJ_AGENT_CONTEXT=0
"$work/codex" sh "$0" --emit "$root" "$thread" "$layout" &
agent_pid=$!
trap 'kill "$agent_pid" 2>/dev/null || true' EXIT
printf 'Goal QA pane %s\n' "$ZELLIJ_PANE_ID"
wait "$agent_pid"
