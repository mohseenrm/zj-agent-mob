#!/bin/sh
# End-to-end tests for scripts/zj-agent-mob-hook.sh.
#
# The hook is the seam between a real agent and the plugin: hook-event JSON
# arrives on stdin, a `zellij pipe --args ...` call comes out. Everything in
# between (event mapping, transcript reading, sanitizing) is untested by the
# Rust suite, which starts from an already-parsed pipe message.
#
# `zellij` is stubbed with a script that records its argv, so these run
# anywhere: no zellij, no agent, no pane. Each case feeds real-shaped JSON and
# asserts on the emitted args.
#
#   ./tests/e2e-hook.sh

set -eu

# shellcheck disable=SC1007  # `CDPATH= cd` clears CDPATH for this command only.
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
HOOK="$ROOT/scripts/zj-agent-mob-hook.sh"

command -v jq >/dev/null 2>&1 || { echo "SKIP: jq not installed"; exit 0; }

PASS=0
FAIL=0

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT INT TERM

# Stub zellij: record argv, one invocation per line. Nothing else is on PATH
# for it, so a hook that shells out to the real zellij would fail loudly.
mkdir -p "$WORK/bin"
cat > "$WORK/bin/zellij" <<'STUB'
#!/bin/sh
printf '%s\n' "$*" >> "$ZJ_TEST_CAPTURE"
STUB
chmod +x "$WORK/bin/zellij"
PATH="$WORK/bin:$PATH"
export PATH

# run <json> [env assignments...] -> prints what the hook sent to zellij
run() {
  _json=$1
  shift
  ZJ_TEST_CAPTURE="$WORK/capture.$$"
  export ZJ_TEST_CAPTURE
  : > "$ZJ_TEST_CAPTURE"
  # Defaults mimic a normal pane; callers override by passing VAR=value.
  env ZELLIJ_PANE_ID=3 ZJ_AGENT_PLUGIN=file:/plugin.wasm \
      ZJ_TEST_CAPTURE="$ZJ_TEST_CAPTURE" "$@" \
      sh "$HOOK" <<EOF >/dev/null 2>&1 || true
$_json
EOF
  cat "$ZJ_TEST_CAPTURE"
}

ok() {
  PASS=$((PASS + 1))
  printf '  ok   %s\n' "$1"
}

bad() {
  FAIL=$((FAIL + 1))
  printf '  FAIL %s\n     %s\n' "$1" "$2"
}

# assert_contains <name> <haystack> <needle>
assert_contains() {
  case "$2" in
    *"$3"*) ok "$1" ;;
    *) bad "$1" "expected to contain '$3', got: ${2:-<no output>}" ;;
  esac
}

# assert_empty <name> <output>
assert_empty() {
  if [ -z "$2" ]; then
    ok "$1"
  else
    bad "$1" "expected no zellij call, got: $2"
  fi
}

echo "event -> status mapping"
assert_contains "SessionStart is idle" \
  "$(run '{"hook_event_name":"SessionStart"}')" "status=idle"
assert_contains "UserPromptSubmit is working" \
  "$(run '{"hook_event_name":"UserPromptSubmit"}')" "status=working"
assert_contains "Notification is waiting" \
  "$(run '{"hook_event_name":"Notification"}')" "status=waiting"
assert_contains "PermissionRequest is waiting (codex)" \
  "$(run '{"hook_event_name":"PermissionRequest"}')" "status=waiting"
assert_contains "Stop is done" \
  "$(run '{"hook_event_name":"Stop"}')" "status=done"
assert_contains "SessionEnd is ended" \
  "$(run '{"hook_event_name":"SessionEnd"}')" "status=ended"
assert_contains "PreToolUse is working" \
  "$(run '{"hook_event_name":"PreToolUse"}')" "status=working"
assert_contains "PostToolUse is working" \
  "$(run '{"hook_event_name":"PostToolUse"}')" "status=working"

echo
echo "events that must stay silent"
assert_empty "unknown event is ignored" \
  "$(run '{"hook_event_name":"SomethingElse"}')"
assert_empty "missing event name is ignored" \
  "$(run '{"session_id":"x"}')"
assert_empty "empty stdin is ignored" \
  "$(run '')"
assert_empty "malformed json is ignored" \
  "$(run 'not json at all')"
# The pane id is what scopes monitoring to zellij; without it there is no
# pane to report against, so an agent outside zellij must be invisible.
assert_empty "no ZELLIJ_PANE_ID means no report" \
  "$(run '{"hook_event_name":"Stop"}' ZELLIJ_PANE_ID=)"
# Documented as halving hook volume: tool events stop reporting entirely.
assert_empty "heartbeat=0 silences PreToolUse" \
  "$(run '{"hook_event_name":"PreToolUse"}' ZJ_AGENT_HEARTBEAT=0)"
assert_empty "heartbeat=0 silences PostToolUse" \
  "$(run '{"hook_event_name":"PostToolUse"}' ZJ_AGENT_HEARTBEAT=0)"
assert_contains "heartbeat=0 still reports turn boundaries" \
  "$(run '{"hook_event_name":"Stop"}' ZJ_AGENT_HEARTBEAT=0)" "status=done"

echo
echo "fields passed through"
out=$(run '{"hook_event_name":"Stop","session_id":"sess-1","cwd":"/home/me/api"}')
assert_contains "pane id" "$out" "pane_id=3"
assert_contains "session id" "$out" "session_id=sess-1"
assert_contains "cwd" "$out" "cwd=/home/me/api"
assert_contains "default tool is claude" "$out" "tool=claude"
assert_contains "tool override" \
  "$(run '{"hook_event_name":"Stop"}' ZJ_AGENT_TOOL=codex)" "tool=codex"
assert_contains "plugin path override" \
  "$(run '{"hook_event_name":"Stop"}' ZJ_AGENT_PLUGIN=file:/custom.wasm)" \
  "--plugin file:/custom.wasm"
assert_contains "tool_name becomes detail" \
  "$(run '{"hook_event_name":"PreToolUse","tool_name":"Edit"}')" "detail=Edit"

echo
echo "claude transcript summaries"
TR="$WORK/transcript.jsonl"
printf '%s\n' \
  '{"type":"last-prompt","lastPrompt":"the fallback prompt"}' \
  '{"type":"ai-title","aiTitle":"Add retry to webhook client"}' > "$TR"
assert_contains "ai-title is preferred" \
  "$(run "{\"hook_event_name\":\"Stop\",\"transcript_path\":\"$TR\"}")" \
  "task=Add retry to webhook client"

printf '%s\n' '{"type":"last-prompt","lastPrompt":"the fallback prompt"}' > "$TR"
assert_contains "falls back to last-prompt" \
  "$(run "{\"hook_event_name\":\"Stop\",\"transcript_path\":\"$TR\"}")" \
  "task=the fallback prompt"

# The newest title wins: the plugin shows current work, not the session's first.
printf '%s\n' \
  '{"type":"ai-title","aiTitle":"older title"}' \
  '{"type":"ai-title","aiTitle":"newest title"}' > "$TR"
assert_contains "latest ai-title wins" \
  "$(run "{\"hook_event_name\":\"Stop\",\"transcript_path\":\"$TR\"}")" \
  "task=newest title"

# Tool events fire constantly against multi-MB transcripts, so they must not
# read them; the plugin treats an empty task as "leave unchanged".
printf '%s\n' '{"type":"ai-title","aiTitle":"should not be read"}' > "$TR"
assert_contains "tool events send an empty task" \
  "$(run "{\"hook_event_name\":\"PreToolUse\",\"transcript_path\":\"$TR\"}")" \
  "task=,"

assert_contains "missing transcript is survivable" \
  "$(run '{"hook_event_name":"Stop","transcript_path":"/no/such/file.jsonl"}')" \
  "status=done"

# A transcript whose tail is unparseable must not take the status report down.
printf '%s\n' 'garbage {{{ not json' > "$TR"
assert_contains "unparseable transcript still reports status" \
  "$(run "{\"hook_event_name\":\"Stop\",\"transcript_path\":\"$TR\"}")" \
  "status=done"

echo
echo "codex transcript summaries"
CX="$WORK/codex"
mkdir -p "$CX/sessions/2026/08/06"
printf '%s\n' \
  '{"type":"event_msg","payload":{"type":"user_message","message":"Bump deps"}}' \
  > "$CX/sessions/2026/08/06/rollout-2026-08-06-sess-9.jsonl"
# Stop now takes its summary from the payload, so the rollout is read on the
# turn-opening events instead.
assert_contains "codex reads the session rollout" \
  "$(run '{"hook_event_name":"UserPromptSubmit","session_id":"sess-9"}' \
      ZJ_AGENT_TOOL=codex CODEX_HOME="$CX")" \
  "task=Bump deps"
assert_contains "codex without a rollout still reports" \
  "$(run '{"hook_event_name":"Stop","session_id":"absent"}' \
      ZJ_AGENT_TOOL=codex CODEX_HOME="$CX")" \
  "status=done"

echo
echo "sanitizing (--args is comma-separated, so commas and newlines break it)"
printf '%s\n' '{"type":"ai-title","aiTitle":"fix a, b and c"}' > "$TR"
out=$(run "{\"hook_event_name\":\"Stop\",\"transcript_path\":\"$TR\"}")
# Every comma in --args must separate two key=value pairs. A comma surviving
# inside a value leaves a fragment with no `=`, which the plugin reads as a new
# key. Asserting the invariant rather than a pair count keeps this test honest
# as fields are added.
args=$(printf '%s' "$out" | sed -n 's/.*--args \(.*\)/\1/p' | tr -d "'")
bad_field=$(printf '%s' "$args" | tr ',' '\n' | grep -v '=' || true)
if [ -z "$bad_field" ]; then
  ok "commas in the task are stripped"
else
  bad "commas in the task are stripped" "fragment without a key: [$bad_field] in: $out"
fi

printf '%s\n' '{"type":"ai-title","aiTitle":"line one\nline two"}' > "$TR"
out=$(run "{\"hook_event_name\":\"Stop\",\"transcript_path\":\"$TR\"}")
if [ "$(printf '%s' "$out" | wc -l | tr -d ' ')" = "0" ]; then
  ok "newlines in the task are stripped"
else
  bad "newlines in the task are stripped" "multi-line args: $out"
fi

# 60 chars is the documented cap; the panel truncates for display anyway.
long=$(printf 'x%.0s' $(seq 1 200))
printf '%s\n' "{\"type\":\"ai-title\",\"aiTitle\":\"$long\"}" > "$TR"
out=$(run "{\"hook_event_name\":\"Stop\",\"transcript_path\":\"$TR\"}")
task_field=${out#*task=}
task_field=${task_field%%,*}
if [ "${#task_field}" -le 60 ]; then
  ok "long tasks are capped at 60 chars"
else
  bad "long tasks are capped at 60 chars" "got ${#task_field} chars"
fi

# A quote or $(...) in a title must not escape into the shell: the hook evals
# jq's @sh output, so this is the injection path that matters.
# shellcheck disable=SC2016  # The payload must reach the hook unexpanded.
printf '%s\n' '{"type":"ai-title","aiTitle":"it'"'"'s $(touch /tmp/zj-pwned) `id`"}' > "$TR"
out=$(run "{\"hook_event_name\":\"Stop\",\"transcript_path\":\"$TR\"}")
if [ -e /tmp/zj-pwned ]; then
  rm -f /tmp/zj-pwned
  bad "shell metacharacters are not executed" "command substitution ran"
else
  ok "shell metacharacters are not executed"
fi
assert_contains "quoted task still reports" "$out" "status=done"

# cwd is interpolated the same way and is attacker-influenced via directory names.
# shellcheck disable=SC2016  # The payload must reach the hook unexpanded.
out=$(run '{"hook_event_name":"Stop","cwd":"/tmp/a b$(touch /tmp/zj-pwned2)"}')
if [ -e /tmp/zj-pwned2 ]; then
  rm -f /tmp/zj-pwned2
  bad "cwd metacharacters are not executed" "command substitution ran"
else
  ok "cwd metacharacters are not executed"
fi

echo
echo "the hook must never break an agent turn"
# Claude aborts nothing on a non-zero hook, but a hung or failing hook is still
# a bad neighbour: the contract in the header is "always exit 0".
for ev in SessionStart UserPromptSubmit Notification Stop SessionEnd Bogus; do
  ZJ_TEST_CAPTURE="$WORK/rc" ; export ZJ_TEST_CAPTURE ; : > "$ZJ_TEST_CAPTURE"
  rc=0
  printf '{"hook_event_name":"%s"}' "$ev" \
    | env ZELLIJ_PANE_ID=3 sh "$HOOK" >/dev/null 2>&1 || rc=$?
  [ "$rc" = "0" ] || break
done
if [ "$rc" = "0" ]; then
  ok "exits 0 for every event"
else
  bad "exits 0 for every event" "exit $rc"
fi

# Even with zellij missing entirely, the hook must succeed silently.
rc=0
printf '{"hook_event_name":"Stop"}' \
  | env PATH="/usr/bin:/bin" ZELLIJ_PANE_ID=3 sh "$HOOK" >/dev/null 2>&1 || rc=$?
if [ "$rc" = "0" ]; then
  ok "exits 0 when zellij is absent"
else
  bad "exits 0 when zellij is absent" "exit $rc"
fi

echo
echo "debug logging"
LOGHOME="$WORK/home"
mkdir -p "$LOGHOME"
run '{"hook_event_name":"Stop"}' ZJ_AGENT_DEBUG=1 HOME="$LOGHOME" >/dev/null
if [ -s "$LOGHOME/.cache/zj-agent-mob/hook.log" ]; then
  ok "debug=1 writes hook.log"
else
  bad "debug=1 writes hook.log" "no log at $LOGHOME/.cache/zj-agent-mob/hook.log"
fi
LOGHOME2="$WORK/home2"
mkdir -p "$LOGHOME2"
run '{"hook_event_name":"Stop"}' HOME="$LOGHOME2" >/dev/null
if [ -e "$LOGHOME2/.cache/zj-agent-mob/hook.log" ]; then
  bad "debug off writes nothing" "log created without ZJ_AGENT_DEBUG=1"
else
  ok "debug off writes nothing"
fi

echo
echo "richer payload fields"

# F1: the tool argument, not just the tool name.
assert_contains "tool_input.file_path reaches detail" \
  "$(run '{"hook_event_name":"PreToolUse","tool_name":"Edit","tool_input":{"file_path":"src/webhook.rs"}}')" \
  "detail=Edit src/webhook.rs"
assert_contains "tool_input.command reaches detail" \
  "$(run '{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"cargo test"}}')" \
  "detail=Bash cargo test"
assert_contains "a tool with no known argument still reports its name" \
  "$(run '{"hook_event_name":"PreToolUse","tool_name":"Glob","tool_input":{}}')" \
  "detail=Glob"

# F3: Stop carries the closing message, so no transcript read is needed.
assert_contains "Stop takes its task from last_assistant_message" \
  "$(run '{"hook_event_name":"Stop","last_assistant_message":"Found 3 issues in the render path"}')" \
  "task=Found 3 issues in the render path"
assert_contains "only the first line of the closing message is used" \
  "$(run '{"hook_event_name":"Stop","last_assistant_message":"first line\nsecond line"}')" \
  "task=first line"

# F2: a real prompt is `waiting`; an idle nudge is not the same thing.
assert_contains "permission_prompt maps to waiting" \
  "$(run '{"hook_event_name":"Notification","notification_type":"permission_prompt","message":"Bash wants to run rm -rf"}')" \
  "status=waiting"
assert_contains "notification text reaches detail" \
  "$(run '{"hook_event_name":"Notification","notification_type":"permission_prompt","message":"Bash wants to run rm -rf"}')" \
  "detail=Bash wants to run rm -rf"
assert_contains "idle_prompt maps to idlewait" \
  "$(run '{"hook_event_name":"Notification","notification_type":"idle_prompt","message":"waiting for input"}')" \
  "status=idlewait"

# F4: a stopped agent must not keep reporting `working`.
assert_contains "StopFailure maps to failed" \
  "$(run '{"hook_event_name":"StopFailure","error_type":"rate_limit","error_message":"rate limited, retry in 30s"}')" \
  "status=failed"
assert_contains "the failure reason reaches detail" \
  "$(run '{"hook_event_name":"StopFailure","error_type":"rate_limit","error_message":"rate limited, retry in 30s"}')" \
  "detail=rate limited"
assert_contains "a failure with no message falls back to the type" \
  "$(run '{"hook_event_name":"StopFailure","error_type":"overloaded"}')" \
  "detail=overloaded"

# F6
assert_contains "PreCompact maps to compact" \
  "$(run '{"hook_event_name":"PreCompact","trigger":"auto"}')" "status=compact"
assert_contains "compaction names its trigger" \
  "$(run '{"hook_event_name":"PreCompact","trigger":"manual"}')" "detail=compacting context (manual)"
assert_contains "PostCompact returns to working" \
  "$(run '{"hook_event_name":"PostCompact","trigger":"auto"}')" "status=working"

# F7: `default` is the common case and must stay off the row.
assert_contains "a risky permission mode is forwarded" \
  "$(run '{"hook_event_name":"UserPromptSubmit","permission_mode":"bypassPermissions"}')" \
  "perm_mode=bypassPermissions"
assert_contains "the default permission mode is suppressed" \
  "$(run '{"hook_event_name":"UserPromptSubmit","permission_mode":"default"}')" \
  "perm_mode=,"

# F5 / F8: counter events carry a delta and no status, so they never
# overwrite the parent pane's own state.
sub=$(run '{"hook_event_name":"SubagentStart","agent_type":"Explore"}')
assert_contains "SubagentStart sends a positive delta" "$sub" "subagent_delta=1"
assert_contains "SubagentStart names the agent type" "$sub" "agent_type=Explore"
assert_contains "SubagentStart carries no status" "$sub" "status=,"
assert_contains "SubagentStop sends a negative delta" \
  "$(run '{"hook_event_name":"SubagentStop","agent_type":"Explore"}')" "subagent_delta=-1"
assert_contains "TaskCreated sends a task delta" \
  "$(run '{"hook_event_name":"TaskCreated","task_title":"Write the tests"}')" "task_delta=1"
assert_contains "TaskCompleted sends a done delta" \
  "$(run '{"hook_event_name":"TaskCompleted"}')" "task_done_delta=1"

# The heartbeat switch has to cover the new chatty events too, or turning it
# off still leaves a fan-out firing several times a second.
for ev in SubagentStart SubagentStop TaskCreated TaskCompleted PostToolUseFailure; do
  assert_empty "heartbeat=0 silences $ev" \
    "$(run "{\"hook_event_name\":\"$ev\"}" ZJ_AGENT_HEARTBEAT=0)"
done

# Shell metacharacters in the new fields must not reach the shell. The single
# quotes are deliberate: the payload must stay unexpanded so the hook is what
# decides whether it ever runs.
# shellcheck disable=SC2016
assert_contains "injection through the notification message is neutralised" \
  "$(run '{"hook_event_name":"Notification","notification_type":"permission_prompt","message":"$(touch /tmp/zj-pwned)"}')" \
  "status=waiting"
if [ -e /tmp/zj-pwned ]; then
  bad "injection through the notification message is neutralised" "command executed"
  rm -f /tmp/zj-pwned
fi

echo
echo "answering permission prompts from the panel (opt-in)"

# The default must stay non-blocking: an unconfigured install can never have a
# hook that waits on a panel the user may not even have open.
perm='{"hook_event_name":"PermissionRequest","tool_name":"Bash","tool_input":{"command":"rm -rf node_modules"}}'
out=$(run "$perm")
assert_contains "PermissionRequest still reports waiting" "$out" "status=waiting"
case "$out" in
  *agent-ask*) bad "approval is off by default" "sent agent-ask without ZJ_AGENT_APPROVE=1" ;;
  *) ok "approval is off by default" ;;
esac

# With the flag on, the hook parks a prompt and waits for a verdict.
out=$(run "$perm" ZJ_AGENT_APPROVE=1 ZJ_AGENT_APPROVE_TIMEOUT=1)
assert_contains "approval mode sends agent-ask" "$out" "agent-ask"
assert_contains "the ask names a verdict file" "$out" "verdict_file="
assert_contains "the ask carries the command being approved" "$out" "tool_arg=rm -rf node_modules"

# Timing out must fall through to the agent's own prompt, not emit a decision.
stdout_only() {
  ZJ_TEST_CAPTURE="$WORK/capture.$$"
  export ZJ_TEST_CAPTURE
  : > "$ZJ_TEST_CAPTURE"
  env ZELLIJ_PANE_ID=3 ZJ_AGENT_PLUGIN=file:/plugin.wasm \
      ZJ_TEST_CAPTURE="$ZJ_TEST_CAPTURE" "$@" \
      sh "$HOOK" <<EOF 2>/dev/null || true
$perm
EOF
}
decision=$(stdout_only ZJ_AGENT_APPROVE=1 ZJ_AGENT_APPROVE_TIMEOUT=1)
if [ -z "$decision" ]; then
  ok "a timeout emits no decision and falls through"
else
  bad "a timeout emits no decision and falls through" "got: $decision"
fi

# A verdict dropped by the panel becomes the documented decision JSON. The
# hook clears the file first, so it is planted by a helper racing the poll.
approve_with() {
  _want=$1
  ZJ_TEST_CAPTURE="$WORK/capture.$$"
  export ZJ_TEST_CAPTURE
  : > "$ZJ_TEST_CAPTURE"
  _vf="${TMPDIR:-/tmp}/zj-agent-mob/verdict.${ZELLIJ_SESSION_NAME:-}.3"
  ( sleep 1; mkdir -p "$(dirname "$_vf")"; printf '%s' "$_want" > "$_vf" ) &
  _planter=$!
  env ZELLIJ_PANE_ID=3 ZJ_AGENT_PLUGIN=file:/plugin.wasm \
      ZELLIJ_SESSION_NAME="${ZELLIJ_SESSION_NAME:-}" \
      ZJ_TEST_CAPTURE="$ZJ_TEST_CAPTURE" ZJ_AGENT_APPROVE=1 ZJ_AGENT_APPROVE_TIMEOUT=8 \
      sh "$HOOK" <<EOF 2>/dev/null || true
$perm
EOF
  wait "$_planter" 2>/dev/null || true
}
assert_contains "an allow verdict becomes an allow decision" \
  "$(approve_with allow)" '"behavior":"allow"'
assert_contains "a deny verdict becomes a deny decision" \
  "$(approve_with deny)" '"behavior":"deny"'

# A blocking hook that can exit non-zero would fail the tool call outright.
if env ZELLIJ_PANE_ID=3 ZJ_AGENT_APPROVE=1 ZJ_AGENT_APPROVE_TIMEOUT=1 \
       ZJ_TEST_CAPTURE="$WORK/capture.exit" sh "$HOOK" <<EOF >/dev/null 2>&1
$perm
EOF
then
  ok "the approval path still exits 0"
else
  bad "the approval path still exits 0" "non-zero exit would fail the tool call"
fi

echo
echo "cross-session identity"

# Pane ids repeat across sessions, so every message says which one it is from.
out=$(run '{"hook_event_name":"Stop"}' ZELLIJ_SESSION_NAME=mob)
assert_contains "the report names its session" "$out" "session=mob"

# The name reaches a file path and a comma-separated arg string, so anything
# that would split either is folded first. src/agent.rs mirrors this exactly.
out=$(run '{"hook_event_name":"Stop"}' ZELLIJ_SESSION_NAME="my session")
assert_contains "spaces in a session name are folded" "$out" "session=my_session"
out=$(run '{"hook_event_name":"Stop"}' ZELLIJ_SESSION_NAME="a,b=c")
assert_contains "a comma cannot split the args string" "$out" "session=a_b_c"
case "$out" in
  *"session=a,b"*) bad "a comma cannot inject an arg" "raw comma survived: $out" ;;
  *) ok "a comma cannot inject an arg" ;;
esac
out=$(run '{"hook_event_name":"Stop"}' ZELLIJ_SESSION_NAME="../evil")
sess=$(printf '%s' "$out" | tr ',' '\n' | grep '^session=' || true)
case "$sess" in
  *"/"*) bad "a session name cannot traverse paths" "slash survived: $sess" ;;
  *) ok "a session name cannot traverse paths" ;;
esac

# The correctness fix: two sessions sharing a pane number must not share a
# verdict file, or approving one answers the other's prompt.
perm_ask='{"hook_event_name":"PermissionRequest","tool_name":"Bash","tool_input":{"command":"rm -rf node_modules"}}'
a=$(run "$perm_ask" ZJ_AGENT_APPROVE=1 ZJ_AGENT_APPROVE_TIMEOUT=1 ZELLIJ_SESSION_NAME=mob)
b=$(run "$perm_ask" ZJ_AGENT_APPROVE=1 ZJ_AGENT_APPROVE_TIMEOUT=1 ZELLIJ_SESSION_NAME=other)
va=$(printf '%s' "$a" | tr ',' '\n' | grep '^verdict_file=' || true)
vb=$(printf '%s' "$b" | tr ',' '\n' | grep '^verdict_file=' || true)
assert_contains "the verdict file is session-qualified" "$va" "verdict.mob.3"
if [ -n "$va" ] && [ "$va" != "$vb" ]; then
  ok "same pane in two sessions gets two verdict files"
else
  bad "same pane in two sessions gets two verdict files" "both were: $va"
fi

# ---------------------------------------------------------------------------
# The cross-session status spool.
#
# The pipe above only reaches the agent's own session, so a panel elsewhere
# reads status from these files instead. The write must be a plain filesystem
# side effect: no subprocess, and never able to block a turn.
# ---------------------------------------------------------------------------

SPOOL="$WORK/spool"

# spool_run <json> [env...] -> runs the hook with the spool pointed at $SPOOL
spool_run() {
  _j=$1
  shift
  run "$_j" ZJ_AGENT_SPOOL_DIR="$SPOOL" "$@" >/dev/null
}

rm -rf "$SPOOL"
spool_run '{"hook_event_name":"UserPromptSubmit","session_id":"uuid-1","cwd":"/Users/x/Projects/web"}' \
  ZELLIJ_SESSION_NAME=mob
if [ -f "$SPOOL/mob.3" ]; then
  ok "a status event writes a spool record"
else
  bad "a status event writes a spool record" "no file at $SPOOL/mob.3: $(ls -A "$SPOOL" 2>/dev/null)"
fi

rec=$(cat "$SPOOL/mob.3" 2>/dev/null || true)
assert_contains "the record carries a timestamp" "$rec" "ts="
assert_contains "the record carries its pane" "$rec" "pane_id=3"
assert_contains "the record carries its session" "$rec" "session=mob"
assert_contains "the record carries the status" "$rec" "status=working"
# Pane ids are recycled, so this is what stops a stale record colouring a new
# agent on the same pane.
assert_contains "the record carries the agent session id" "$rec" "session_id=uuid-1"

# The plugin keys identity off the filename, so a record must agree with it.
name_pane=$(printf '%s' "$rec" | tr ',' '\n' | grep '^pane_id=' | cut -d= -f2)
name_sess=$(printf '%s' "$rec" | tr ',' '\n' | grep '^session=' | cut -d= -f2)
if [ "$name_sess.$name_pane" = "mob.3" ]; then
  ok "the record agrees with its filename"
else
  bad "the record agrees with its filename" "got $name_sess.$name_pane for file mob.3"
fi

# One line only: the plugin reads the first and treats extra lines as malformed.
lines=$(wc -l < "$SPOOL/mob.3" | tr -d ' ')
if [ "$lines" = "1" ]; then
  ok "the record is exactly one line"
else
  bad "the record is exactly one line" "got $lines lines"
fi

# On a shared /tmp another user must not be able to read prompt text.
# GNU `stat -f` means "filesystem info" and exits 0 printing a block dump, so a
# `-f || -c` fallback silently takes the wrong branch on Linux. Pick by probing
# for GNU first instead of relying on a failure that never comes.
if stat -c '%a' . >/dev/null 2>&1; then
  mode=$(stat -c '%a' "$SPOOL" 2>/dev/null || echo "?")
else
  mode=$(stat -f '%Lp' "$SPOOL" 2>/dev/null || echo "?")
fi
if [ "$mode" = "700" ]; then
  ok "the spool directory is private (0700)"
else
  bad "the spool directory is private (0700)" "mode was $mode"
fi

# The rename into place is what publishes a record, so a reader never sees a
# partial one and no debris is left behind.
leftover=$(find "$SPOOL" -name '*.tmp' 2>/dev/null | wc -l | tr -d ' ')
if [ "$leftover" = "0" ]; then
  ok "no partial .tmp file is left behind"
else
  bad "no partial .tmp file is left behind" "$leftover found"
fi

# A newer event replaces the record rather than appending: the spool is a
# snapshot per agent, so it cannot grow without bound.
spool_run '{"hook_event_name":"Stop","last_assistant_message":"all done"}' ZELLIJ_SESSION_NAME=mob
rec2=$(cat "$SPOOL/mob.3" 2>/dev/null || true)
assert_contains "a later event overwrites the record" "$rec2" "status=done"
lines2=$(wc -l < "$SPOOL/mob.3" | tr -d ' ')
if [ "$lines2" = "1" ]; then
  ok "overwriting does not append"
else
  bad "overwriting does not append" "got $lines2 lines"
fi

# SessionEnd retires the agent, so the record must not outlive it and colour a
# recycled pane id later.
spool_run '{"hook_event_name":"SessionEnd","reason":"logout"}' ZELLIJ_SESSION_NAME=mob
if [ -f "$SPOOL/mob.3" ]; then
  bad "SessionEnd removes the record" "file still present"
else
  ok "SessionEnd removes the record"
fi

# Two sessions on the same pane number are different agents and need different
# files, exactly like the verdict path.
rm -rf "$SPOOL"
spool_run '{"hook_event_name":"UserPromptSubmit"}' ZELLIJ_SESSION_NAME=mob
spool_run '{"hook_event_name":"UserPromptSubmit"}' ZELLIJ_SESSION_NAME=other
if [ -f "$SPOOL/mob.3" ] && [ -f "$SPOOL/other.3" ]; then
  ok "same pane in two sessions gets two records"
else
  bad "same pane in two sessions gets two records" "$(ls -A "$SPOOL" 2>/dev/null)"
fi

# A session name that could split the args or escape the directory is folded
# before it reaches a path.
rm -rf "$SPOOL"
spool_run '{"hook_event_name":"UserPromptSubmit"}' ZELLIJ_SESSION_NAME="../evil"
if [ -f "$SPOOL/.._evil.3" ]; then
  ok "a session name cannot escape the spool directory"
else
  bad "a session name cannot escape the spool directory" "$(find "$SPOOL" -type f 2>/dev/null)"
fi

# Opt-out must be complete: no directory, no file, no error.
rm -rf "$SPOOL"
out=$(run '{"hook_event_name":"UserPromptSubmit"}' ZJ_AGENT_SPOOL_DIR="$SPOOL" ZJ_AGENT_SPOOL=0 ZELLIJ_SESSION_NAME=mob)
if [ -e "$SPOOL" ]; then
  bad "ZJ_AGENT_SPOOL=0 writes nothing" "$(ls -A "$SPOOL")"
else
  ok "ZJ_AGENT_SPOOL=0 writes nothing"
fi
assert_contains "opting out still pipes to its own session" "$out" "status=working"

# An unwritable spool must degrade to the pipe, never fail the turn: the hook's
# contract is that the worst case is the normal experience.
rm -rf "$SPOOL"
mkdir -p "$SPOOL"
chmod 500 "$SPOOL"
out=$(run '{"hook_event_name":"UserPromptSubmit"}' ZJ_AGENT_SPOOL_DIR="$SPOOL/nested" ZELLIJ_SESSION_NAME=mob)
assert_contains "an unwritable spool still pipes" "$out" "status=working"
chmod 700 "$SPOOL"
rm -rf "$SPOOL"

# Every agent event writes, not just turn boundaries, or a panel elsewhere
# would only ever see the start and end of a turn.
rm -rf "$SPOOL"
spool_run '{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"cargo test"}}' \
  ZELLIJ_SESSION_NAME=mob
rec3=$(cat "$SPOOL/mob.3" 2>/dev/null || true)
assert_contains "a tool event spools its detail" "$rec3" "detail=Bash cargo test"

# The heartbeat opt-out must suppress the spool write too, or the escape hatch
# only halves the work it promises to remove.
rm -rf "$SPOOL"
spool_run '{"hook_event_name":"PreToolUse","tool_name":"Bash"}' ZJ_AGENT_HEARTBEAT=0 ZELLIJ_SESSION_NAME=mob
if [ -e "$SPOOL/mob.3" ]; then
  bad "ZJ_AGENT_HEARTBEAT=0 also skips the spool" "record written anyway"
else
  ok "ZJ_AGENT_HEARTBEAT=0 also skips the spool"
fi

# A record is a whole snapshot, so an event that carries no session_id must
# inherit the last known one rather than blanking it. Without this a recycled
# pane id could inherit a dead agent's status, which the plugin cannot detect.
rm -rf "$SPOOL"
spool_run '{"hook_event_name":"UserPromptSubmit","session_id":"uuid-keep","cwd":"/Users/x/Projects/web"}' \
  ZELLIJ_SESSION_NAME=mob
spool_run '{"hook_event_name":"Notification","type":"permission_prompt","message":"needs permission"}' \
  ZELLIJ_SESSION_NAME=mob
rec4=$(cat "$SPOOL/mob.3" 2>/dev/null || true)
assert_contains "an event without a session id inherits the last one" "$rec4" "session_id=uuid-keep"
assert_contains "an event without a cwd inherits the last one" "$rec4" "cwd=/Users/x/Projects/web"
assert_contains "the inheriting event still records its own status" "$rec4" "status=waiting"

# Deltas describe one moment and are replayed on every poll, so a snapshot must
# not carry them or the counts would inflate without bound.
rm -rf "$SPOOL"
spool_run '{"hook_event_name":"SubagentStart","agent_type":"Explore"}' ZELLIJ_SESSION_NAME=mob
spool_run '{"hook_event_name":"UserPromptSubmit","session_id":"u"}' ZELLIJ_SESSION_NAME=mob
rec5=$(cat "$SPOOL/mob.3" 2>/dev/null || true)
case "$rec5" in
  *subagent_delta*|*task_delta*|*task_done_delta*)
    bad "a snapshot carries no deltas" "found a delta field: $rec5" ;;
  *) ok "a snapshot carries no deltas" ;;
esac

# The pane id reaches a file path and the args string. Zellij only ever sets a
# bare integer, so anything else is a broken or planted environment.
rm -rf "$SPOOL"
out=$(run '{"hook_event_name":"UserPromptSubmit"}' ZJ_AGENT_SPOOL_DIR="$SPOOL" ZELLIJ_PANE_ID='1;rm -rf /' ZELLIJ_SESSION_NAME=mob)
assert_empty "a non-numeric pane id reports nothing" "$out"
if [ -e "$SPOOL" ]; then
  bad "a non-numeric pane id writes no record" "$(find "$SPOOL" -type f)"
else
  ok "a non-numeric pane id writes no record"
fi
out=$(run '{"hook_event_name":"UserPromptSubmit"}' ZJ_AGENT_SPOOL_DIR="$SPOOL" ZELLIJ_PANE_ID='../x' ZELLIJ_SESSION_NAME=mob)
assert_empty "a pane id cannot traverse paths" "$out"

# An agent outside zellij has no pane to attribute a record to.
rm -rf "$SPOOL"
run '{"hook_event_name":"UserPromptSubmit"}' ZJ_AGENT_SPOOL_DIR="$SPOOL" ZELLIJ_PANE_ID= >/dev/null
if [ -e "$SPOOL" ]; then
  bad "no pane id writes no record" "$(ls -A "$SPOOL")"
else
  ok "no pane id writes no record"
fi

echo
echo "-------------------------------------------"
printf '%d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
