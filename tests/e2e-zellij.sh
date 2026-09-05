#!/bin/sh
# End-to-end tests against a REAL zellij running the REAL wasm plugin.
#
# Every other suite stops at a stub: tests/hook_e2e.rs runs the real hook against
# a fake `zellij`, and the Rust unit tests run the real state machine against
# synthetic pipe messages. Nothing else loads the compiled wasm into a running
# zellij, so nothing else can catch a load failure, a wasm-only panic, or a
# render that overflows the pane it was given.
#
# These drive a throwaway detached session and read the panel back with
# `dump-screen`, which is the only way to see what the user actually sees.
#
#   ./tests/e2e-zellij.sh
#
# Skips cleanly when zellij or the wasm is missing, so CI is unaffected.

set -eu

# shellcheck disable=SC1007  # `CDPATH= cd` clears CDPATH for this command only.
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
WASM="$ROOT/target/wasm32-wasip1/release/zj-agent-mob.wasm"

command -v zellij >/dev/null 2>&1 || { echo "SKIP: zellij not installed"; exit 0; }
[ -f "$WASM" ] || { echo "SKIP: no wasm at $WASM (cargo build --release --target wasm32-wasip1)"; exit 0; }
command -v script >/dev/null 2>&1 || { echo "SKIP: script(1) needed to run zellij headless"; exit 0; }

PASS=0
FAIL=0
ok()  { PASS=$((PASS + 1)); printf '  ok   %s\n' "$1"; }
bad() { FAIL=$((FAIL + 1)); printf '  FAIL %s\n     %s\n' "$1" "$2"; }

WORK=$(mktemp -d)
SESSION="zjqa-$$"

# The panel must never be driven by the developer's own zellij session, and the
# hook inherits these: unset so a nested run cannot report into the real panel.
unset ZELLIJ ZELLIJ_SESSION_NAME ZELLIJ_PANE_ID

cleanup() {
  zellij delete-session "$SESSION" --force >/dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap cleanup EXIT INT TERM

CONF="$WORK/config"
mkdir -p "$CONF/plugins"
cp "$WASM" "$CONF/plugins/zj-agent-mob.wasm"
PLUGIN="file:$CONF/plugins/zj-agent-mob.wasm"

# A tiny config: no startup tips, no pane frames. Frames would put a border in
# every dump and make column assertions meaningless.
cat > "$CONF/config.kdl" <<'EOF'
show_startup_tips false
pane_frames false
EOF

zj() { zellij --session "$SESSION" "$@"; }

# dump [pane] -> the panel's visible text on stdout
dump() {
  zj action dump-screen --path "$WORK/screen.txt" >/dev/null 2>&1 || true
  cat "$WORK/screen.txt" 2>/dev/null || true
}

# Fires the real hook from inside a real pane of this session.
#
# The pane id has to be one zellij actually has: a row whose pane is not in the
# PaneManifest is dropped by `reconcile` as soon as the next update arrives, so
# a synthetic id produces a row that vanishes before it can be asserted on.
# Running the hook through `zellij run` is what gives it a genuine
# $ZELLIJ_PANE_ID, exactly as an agent in that pane would have.
hook() {
  _status_json=$1
  cat > "$WORK/fire.sh" <<FIRE
#!/bin/sh
printf '%s' '$_status_json' \
  | ZJ_AGENT_PLUGIN="$PLUGIN" \
    ZJ_AGENT_SPOOL_DIR="$WORK/spool" \
    ZJ_AGENT_FANOUT=0 \
    sh "$ROOT/scripts/zj-agent-mob-hook.sh"
# The pane must outlive the hook: closing it takes the row with it.
sleep 300
FIRE
  chmod +x "$WORK/fire.sh"
  zj run --close-on-exit -- "$WORK/fire.sh" >/dev/null 2>&1 || true
  sleep 3
  # `run` opens a new pane and focuses it, so bring the panel back on top.
  zj action launch-or-focus-plugin --floating "$PLUGIN" >/dev/null 2>&1 || true
  sleep 2
}

echo "starting a real zellij session"
nohup script -q /dev/null zellij --config-dir "$CONF" --session "$SESSION" \
  >"$WORK/boot.log" 2>&1 &
# Wait for the session to be listed rather than sleeping a fixed time: a loaded
# machine is slower than any constant we would pick.
i=0
while [ "$i" -lt 60 ]; do
  zellij list-sessions 2>/dev/null | grep -q "$SESSION" && break
  i=$((i + 1)); sleep 0.5
done
zellij list-sessions 2>/dev/null | grep -q "$SESSION" \
  || { echo "SKIP: could not start a headless session"; exit 0; }

echo
echo "plugin load"

# The whole point of the `exports` check in scripts/check.sh is that a wrong
# build fails HERE. This asserts it actually loads rather than that the symbol
# table looks right.
# Launched with the DEFAULT configuration on purpose. Zellij keys a plugin
# instance by (url, configuration), and the hook's `zellij pipe --plugin` passes
# no configuration, so a panel launched with one would be a different instance
# and would never receive a single hook message.
#
# The cost is that the process scan is on, so the panel may also list agents the
# developer has running in their own sessions. The assertions below therefore
# look for this session's own rows rather than asserting on the whole screen.
zj action launch-or-focus-plugin --floating "$PLUGIN" >/dev/null 2>&1 || true
sleep 2
screen=$(dump)
case "$screen" in
  *"permission"*|*"Allow?"*) ok "plugin loads and asks for permissions" ;;
  *"zj-agent-mob"*)          ok "plugin loads (permissions already granted)" ;;
  *) bad "plugin loads" "nothing recognisable on screen: ${screen:-<empty>}" ;;
esac

# Granting is the first thing every new user does, and nothing tests that the
# panel recovers from it.
zj action write-chars "y" >/dev/null 2>&1 || true
sleep 2
screen=$(dump)
assert_panel() {
  case "$screen" in
    *"zj-agent-mob"*) ok "$1" ;;
    *) bad "$1" "panel not rendering: ${screen:-<empty>}" ;;
  esac
}
assert_panel "panel renders after the permission grant"

echo
echo "hook -> panel, for real"

# This is the seam the two suites each half-test: the hook's --args string
# meeting the plugin's parser, with no stub on either side.
hook '{"hook_event_name":"UserPromptSubmit","cwd":"/tmp/proj-alpha","session_id":"sess-a"}'
screen=$(dump)
case "$screen" in
  *"claude"*) ok "a real hook event produces a row" ;;
  *) bad "a real hook event produces a row" "no row: $screen" ;;
esac
case "$screen" in
  *"working"*) ok "the row carries the status the hook sent" ;;
  *) bad "the row carries the status the hook sent" "no 'working': $screen" ;;
esac

hook '{"hook_event_name":"Notification","message":"Claude needs your permission to use Bash","notification_type":"permission_prompt","cwd":"/tmp/proj-alpha","session_id":"sess-a"}'
screen=$(dump)
case "$screen" in
  *"waiting"*) ok "a blocked agent shows as waiting" ;;
  *) bad "a blocked agent shows as waiting" "no 'waiting': $screen" ;;
esac
# `y yes  m message` is offered only while the selected row is blocked, so the
# footer proves the block reached the panel even when the pane is too narrow for
# the detail line that spells it out.
case "$screen" in
  *"wants: permission"*|*"y yes"*|*"m message"*)
    ok "the block reason reaches the panel" ;;
  *) bad "the block reason reaches the panel" "not offered the reply keys: $screen" ;;
esac

echo
echo "render geometry"

# Every line the panel prints must fit the pane. A line that exactly fills it
# wraps and eats the row below, which is why content_width() is cols-1 - but
# only the elements that actually truncate honour that. The header was not one
# of them: at a narrow width it ran past the edge and the rule below landed on
# the same row.
#
# Measured in CHARACTERS, not bytes: the rule is 3 bytes per column and would
# fail a byte-wise check at every width.
# The floating pane starts at a fraction of the terminal, and `resize` moves it
# by one column at a time, so the sweep walks it down and back up.
geometry_case() {
  _label=$1
  zj action dump-screen --path "$WORK/g.txt" >/dev/null 2>&1 || true
  _first=$(sed -n 1p "$WORK/g.txt")
  # The rule spans the content width, so it is the pane's own yardstick.
  _rule=$(sed -n 2p "$WORK/g.txt" | awk '{ print length($0) }')

  # The header and the rule must not share a row. A wrapped header is what puts
  # the rule's box-drawing character on the title's line.
  case "$_first" in
    *"zj-agent-mob"*"─"*)
      bad "$_label: header does not collide with the rule" "header wrapped: $_first" ;;
    *) ok "$_label: header does not collide with the rule" ;;
  esac

  # The overflow this caught: the header ran past the pane and the rule wrapped
  # onto its line, making row 1 several times the pane's width.
  _head=$(printf '%s' "$_first" | awk '{ print length($0) }')
  if [ "${_rule:-0}" -gt 0 ] && [ "${_head:-0}" -gt "$_rule" ]; then
    bad "$_label: the header fits the pane" "header $_head columns vs a $_rule-column pane"
  else
    ok "$_label: the header fits the pane"
  fi

  # Every row must fit: a line wider than the rule has wrapped, and a wrap is
  # what pushes the footer off the bottom of the pane. The rule is the pane's
  # own width by construction, so it is the yardstick.
  _over=$(awk -v w="$_rule" '
    w > 0 && length($0) > w { print NR ": " $0 }
  ' "$WORK/g.txt")
  if [ -n "$_over" ]; then
    bad "$_label: every row fits the pane" "wider than the $_rule-column rule: $_over"
  else
    ok "$_label: every row fits the pane"
  fi
}

geometry_case "default width"

i=0
while [ "$i" -lt 10 ]; do
  zj action resize decrease right >/dev/null 2>&1 || true
  i=$((i + 1))
done
sleep 1
geometry_case "narrow"

i=0
while [ "$i" -lt 10 ]; do
  zj action resize increase right >/dev/null 2>&1 || true
  i=$((i + 1))
done
sleep 1

echo
echo "wide text: CJK and a long task"

# Task summaries are arbitrary user text, and the row builder counts characters
# where a terminal counts columns. CJK is the case where those disagree.
hook '{"hook_event_name":"Stop","cwd":"/tmp/proj-cjk","session_id":"sess-c","last_assistant_message":"実装計画を確認して修正する作業を継続しています"}'
geometry_case "cjk task summary"

echo
printf 'passed %d, failed %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
