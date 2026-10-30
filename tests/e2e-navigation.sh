#!/bin/sh
set -eu

# shellcheck disable=SC1007
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
WASM="$ROOT/target/wasm32-wasip1/release/zj-agent-mob.wasm"
for binary in zellij script jq cc; do
  command -v "$binary" >/dev/null 2>&1 || { echo "SKIP: navigation needs $binary"; exit 0; }
done
[ -f "$WASM" ] || { echo "SKIP: build the wasm before navigation checks"; exit 0; }

WORK=$(mktemp -d)
SOURCE="zjn-$$-a"
FOREIGN="zjn-$$-b"
OLD_SOURCE=$SOURCE
OLD_FOREIGN=$FOREIGN
PLUGIN="file:$WORK/plugins/zj-agent-mob.wasm"
export ZJ_AGENT_SPOOL_DIR="$WORK/spool"
unset ZELLIJ ZELLIJ_SESSION_NAME ZELLIJ_PANE_ID

cleanup() {
  if [ "${ZJ_QA_KEEP:-0}" = 1 ]; then
    printf 'QA_WORK=%s\nSOURCE=%s\nFOREIGN=%s\n' "$WORK" "$SOURCE" "$FOREIGN"
    return
  fi
  zellij delete-session "$SOURCE" --force >/dev/null 2>&1 || true
  zellij delete-session "$FOREIGN" --force >/dev/null 2>&1 || true
  if command -v trash >/dev/null 2>&1; then trash "$WORK"; fi
}
trap cleanup EXIT INT TERM

mkdir -p "$WORK/plugins"
cp "$WASM" "$WORK/plugins/zj-agent-mob.wasm"
cc "$ROOT/tests/fixtures/idle-agent.c" -o "$WORK/codex"
cat > "$WORK/config.kdl" <<'CONFIG'
show_startup_tips false
pane_frames false
CONFIG

cat > "$WORK/agent.sh" <<'AGENT'
#!/bin/sh
root=$1
plugin=$2
work=$3
label=$4
emit() {
  printf '{"hook_event_name":"Stop","session_id":"qa-%s","cwd":"/tmp/zj-qa-%s","last_assistant_message":"Which deployment environment?"}' "$label" "$label" |
    ZJ_AGENT_TOOL=codex ZJ_AGENT_PLUGIN="$plugin" ZJ_AGENT_FANOUT=0 sh "$root/scripts/zj-agent-mob-hook.sh"
}
"$work/codex" &
agent_pid=$!
emit
printf 'NAV-TARGET %s\n' "$ZELLIJ_PANE_ID"
while kill -0 "$agent_pid" 2>/dev/null; do
  if [ -f "$work/refire.$label" ]; then
    mv "$work/refire.$label" "$work/refire.$label.used"
    emit
  fi
  sleep 0.2
done
AGENT
chmod +x "$WORK/agent.sh"

fail() { printf 'FAIL: %s\n' "$1" >&2; exit 1; }
ok() { printf '  ok   %s\n' "$1"; }
zj() { zellij --session "$SESSION" "$@"; }
panel() {
  zj action launch-or-focus-plugin --floating "$PLUGIN" >/dev/null 2>&1
  sleep 1
}
boot() {
  SESSION=$1
  nohup script -q /dev/null zellij --config-dir "$WORK" --session "$SESSION" > "$WORK/boot.$SESSION.log" 2>&1 &
  n=0
  while [ "$n" -lt 40 ]; do
    if zj action list-panes --json 2>/dev/null | jq -e 'length > 0' >/dev/null 2>&1; then break; fi
    n=$((n + 1)); sleep 0.2
  done
  [ "$n" -lt 40 ] || fail "headless session did not start: $SESSION"
  panel
  screen=$(zj action dump-screen 2>/dev/null)
  case "$screen" in *permission*|*Allow\?*) zj action write-chars y >/dev/null 2>&1; sleep 1 ;; esac
}
fixture() {
  pane=$(zj run --close-on-exit -- "$WORK/agent.sh" "$ROOT" "$PLUGIN" "$WORK" "$1")
  pane=${pane#terminal_}
  case "$pane" in ''|*[!0-9]*) fail "new pane did not return an ID: $pane" ;; esac
  sleep 1
}
find_and_jump() {
  panel
  zj action write-chars "/zj-qa-$1" >/dev/null 2>&1
  sleep 1
  screen=$(zj action dump-screen 2>/dev/null)
  case "$screen" in *codex*waiting*) ;; *) fail "Codex input request is not waiting: $screen" ;; esac
  zj action write 13 >/dev/null 2>&1
  sleep 1
}
refire_after_rename() {
  n=0
  while [ "$n" -lt 50 ]; do
    if grep -F "$SESSION" "$WORK"/spool/server.* >/dev/null 2>&1; then break; fi
    n=$((n + 1)); sleep 0.2
  done
  [ "$n" -lt 50 ] || fail "the panel did not publish its renamed server route"
  touch "$WORK/refire.$1"
  n=0
  while [ "$n" -lt 40 ] && [ ! -f "$WORK/spool/$SESSION.$pane" ]; do
    n=$((n + 1)); sleep 0.2
  done
  [ -f "$WORK/spool/$SESSION.$pane" ] || fail "an old shell did not report under the renamed session"
}
assert_not_recreated() {
  live=$(zellij list-sessions --no-formatting 2>/dev/null | awk -v name="$1" '$1 == name && $0 !~ /EXITED/ { print "live" }')
  [ -z "$live" ] || fail "jump created the old session: $1"
}

echo 'real-pane navigation after renames'
boot "$SOURCE"
fixture main
main_pane=$pane
zj action rename-pane 'Renamed navigation target' >/dev/null 2>&1
find_and_jump main
screen=$(zj action dump-screen 2>/dev/null)
case "$screen" in *"NAV-TARGET $main_pane"*) ok 'a pane-title rename keeps the exact target' ;; *) fail "jump missed the renamed pane: $screen" ;; esac

zj action rename-session "$SOURCE-r" >/dev/null 2>&1
SOURCE="$SOURCE-r"
SESSION=$SOURCE
refire_after_rename main
ok 'an existing shell reports under the current session name'
zj action start-or-reload-plugin "$PLUGIN" >/dev/null 2>&1
sleep 1
find_and_jump main
screen=$(zj action dump-screen 2>/dev/null)
case "$screen" in *"NAV-TARGET $main_pane"*) ok 'a cold panel jumps to the exact pane after a session rename' ;; *) fail "cold-panel jump missed its target: $screen" ;; esac
assert_not_recreated "$OLD_SOURCE"
ok 'navigation does not recreate the old session'

boot "$FOREIGN"
fixture foreign
foreign_pane=$pane
panel
zj action rename-session "$FOREIGN-r" >/dev/null 2>&1
FOREIGN="$FOREIGN-r"
SESSION=$FOREIGN
refire_after_rename foreign
zj action focus-pane-id terminal_0 >/dev/null 2>&1
SESSION=$SOURCE
find_and_jump foreign
clients=$(zellij --session "$FOREIGN" action list-clients 2>/dev/null)
printf '%s\n' "$clients" | awk -v pane="terminal_$foreign_pane" '$2 == pane { found = 1 } END { exit !found }' \
  || fail "the cross-session client did not land on the expected pane: $clients"
assert_not_recreated "$OLD_FOREIGN"
ok 'cross-session navigation follows the renamed session and exact pane'
echo 'navigation: 5 checks passed'
