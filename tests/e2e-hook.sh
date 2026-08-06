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
assert_contains "codex reads the session rollout" \
  "$(run '{"hook_event_name":"Stop","session_id":"sess-9"}' \
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
# One comma per key=value pair and no more: a comma inside the task would be
# read by the plugin as the start of a new key.
# 7 key=value pairs means exactly 6 separators. A comma surviving in the task
# would push this higher and the plugin would read the tail as a new key.
n=$(printf '%s' "$out" | sed 's/[^,]//g' | tr -d '\n' | wc -c | tr -d ' ')
if [ "$n" = "6" ]; then
  ok "commas in the task are stripped"
else
  bad "commas in the task are stripped" "expected 6 separators, saw $n in: $out"
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
echo "-------------------------------------------"
printf '%d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
