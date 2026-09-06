#!/bin/sh
# Capture the tour as a sequence of panel frames.
#
#   sh scripts/demo/capture-tour.sh <framedir>
#
# No terminal recorder, no ttyd, no headless browser. The tour is driven with
# `zellij action` and the panel's own rendered output is read back with
# `dump-screen --ansi`; each read is one frame. render-frames.py turns those
# frames into the GIF.
#
# Why this rather than recording a terminal: the panel is the entire subject, so
# recording a whole terminal window means fighting everything that is not the
# panel - shell prompts, the session's other panes, the recorder's own
# environment leaking into the session under test. Reading the panel directly
# removes all of it, and has the side benefit that every frame is inspectable as
# text when something looks wrong.
#
# The session is created inside this script with its own pty, so nothing is
# inherited from the caller's shell.
set -e

FRAMES=${1:?usage: capture-tour.sh <framedir>}
SESSION=${ZJ_TOUR_SESSION:-zjtour}
API_SESSION=payments-api
INFRA_SESSION=platform-infra
CFG=/tmp/zj-tour-cfg
WASM="$HOME/.config/zellij/plugins/zj-agent-mob.wasm"
# Discovery ON, unlike the old tour.
#
# `discover=false` also switches off the SPOOL POLL - request_scan() returns
# early on `!self.discover`, and the scan is the only thing that reads the
# cross-session spool. With it off, foreign agents can never appear at all, which
# is most of the panel's point. The reason it used to be off was to keep the
# machine's real agents out of frame, and moving the spool aside for the duration
# already does that, without disabling the feature being demonstrated.
PLUGIN_CONF="discover=true"
REPO=$HOME/src
DIR=$(cd "$(dirname "$0")" && pwd)
MOCK="$DIR/mock-agent.sh"

rm -rf "$FRAMES"
mkdir -p "$FRAMES"
SEQ=0

# Zellij resolves "the current session" from these, so a tour driven from inside
# a Zellij session would have the plugin believe it lives in the OUTER session:
# the panel names the wrong session and every prop session reads `gone`.
unset ZELLIJ ZELLIJ_SESSION_NAME ZELLIJ_PANE_ID
export ZELLIJ_CONFIG_DIR="$CFG"

za() { zellij -s "$SESSION" action "$@" </dev/null; }
za_in() { _s=$1; shift; zellij -s "$_s" action "$@" </dev/null; }

# `zellij pipe` blocks waiting for the plugin to reply, and a hidden panel never
# answers; cap it so one such call cannot stall the tour. Exit 124 is the cap
# firing, which is harmless - the message was delivered, only the reply is
# unheard.
if command -v timeout >/dev/null 2>&1; then TO=timeout
elif command -v gtimeout >/dev/null 2>&1; then TO=gtimeout
else TO=""; fi

pipe_args() {
  _name=$1 _args=$2 _e=0
  if [ -n "$TO" ]; then
    $TO 8 zellij -s "$SESSION" action pipe --name "$_name" --plugin "file:$WASM" \
      --plugin-configuration "$PLUGIN_CONF" --args "$_args" </dev/null >/dev/null 2>&1 || _e=$?
  else
    zellij -s "$SESSION" action pipe --name "$_name" --plugin "file:$WASM" \
      --plugin-configuration "$PLUGIN_CONF" --args "$_args" </dev/null >/dev/null 2>&1 || _e=$?
  fi
  case $_e in 0 | 124) ;; *) echo "pipe $_name: exit $_e" >&2 ;; esac
}
emit() { pipe_args agent-status "$1"; }
emit_ask() { pipe_args agent-ask "$1"; }

# The cross-session spool: one server per session means a foreign agent cannot
# push into this panel, so it writes a record here and panels poll it. The tour
# writes the same records the hook would, by the same route.
SPOOL="${TMPDIR:-/tmp}/zj-agent-mob-$(id -u 2>/dev/null || echo 0)/status"

spool() {
  _sess=$1 _pane=$2 _rest=$3
  [ -d "$SPOOL" ] || { mkdir -p "$SPOOL" 2>/dev/null; chmod 700 "$SPOOL" 2>/dev/null; }
  printf 'ts=%s,pane_id=%s,session=%s,%s\n' \
    "$(date +%s)" "$_pane" "$_sess" "$_rest" > "$SPOOL/$_sess.$_pane.tmp"
  mv -f "$SPOOL/$_sess.$_pane.tmp" "$SPOOL/$_sess.$_pane"
}

# Grab one frame. `$1` is how many frame-times it should stay up, which is how a
# beat is held without capturing the same panel N times.
#
# A plugin pane is only dumpable while it holds focus, and pane churn moves
# focus, so this re-focuses every time.
# Panel geometry, re-asserted before every frame: switching to the install
# screen and back snaps the pane to a default small enough to clip the list.
fill() {
  [ -n "$PANEL" ] || return 0
  za change-floating-pane-coordinates --pane-id "$PANEL" \
    --x 1 --y 1 --width 92 --height 30 >/dev/null 2>&1 || true
}

snap() {
  _hold=${1:-1}
  SEQ=$((SEQ + 1))
  _f=$(printf '%s/%04d' "$FRAMES" "$SEQ")
  za focus-pane-id "$PANEL" >/dev/null 2>&1 || true
  fill
  # NOT `--ansi`: it returns zero bytes for a plugin pane on this Zellij, while
  # working normally for a terminal pane in the same session. The panel is a
  # plugin pane, so its colours cannot be read back at all - render-frames.py
  # re-applies them from the panel's own vocabulary instead.
  za dump-screen --path "$_f.txt" >/dev/null 2>&1 || true
  if [ ! -s "$_f.txt" ]; then
    # A missed dump would otherwise become a blank frame mid-tour.
    SEQ=$((SEQ - 1))
    rm -f "$_f.txt"
    return 0
  fi
  echo "$_hold" > "$_f.hold"
}

# Send a key to the panel and capture the result.
key() {
  za focus-pane-id "$PANEL" >/dev/null 2>&1 || true
  za send-keys "$1" >/dev/null 2>&1 || true
  sleep "${2:-0.35}"
  snap "${3:-2}"
}

# Type a string one key at a time, one frame each, so typing reads as typing.
type_str() {
  _s=$1
  while [ -n "$_s" ]; do
    _c=$(printf '%s' "$_s" | cut -c1)
    _s=$(printf '%s' "$_s" | cut -c2-)
    case $_c in
      ' ') key space 0.16 1 ;;
      *) key "$_c" 0.16 1 ;;
    esac
  done
}

# Poll until the panel has rendered `$1`, so beats are gated on what actually
# appeared rather than on a sleep.
wait_for() {
  _needle=$1 _i=0
  while [ "$_i" -lt 60 ]; do
    za focus-pane-id "$PANEL" >/dev/null 2>&1 || true
    rm -f /tmp/zj-tour-wait.txt
    za dump-screen --path /tmp/zj-tour-wait.txt >/dev/null 2>&1 || true
    if [ -s /tmp/zj-tour-wait.txt ] && grep -q "$_needle" /tmp/zj-tour-wait.txt 2>/dev/null; then
      return 0
    fi
    sleep 0.25
    _i=$((_i + 1))
  done
  echo "wait_for: never saw '$_needle'" >&2
  return 1
}

new_agent() {
  _tool=$1 _task=$2 _prompt=$3
  shift 3
  _steps=""
  for _s in "$@"; do _steps="$_steps '$_s'"; done
  _id=$(za new-pane --name "$_tool" -- sh -c \
    "ZJ_MOCK_PROMPT='$_prompt' sh '$MOCK' '$_tool' '$_task'$_steps" 2>/dev/null \
    | tr -d '[:space:]' | sed 's/^terminal_//')
  case $_id in '' | *[!0-9]*) echo "new_agent: no pane id for $_tool" >&2; exit 1 ;; esac
  sleep 0.6
  echo "$_id"
}

# Where the machine's real spool records are parked for the duration. Declared
# before the trap so an early failure still restores them.
#
# The spool path is fixed: the plugin's scan reads ZJ_AGENT_SPOOL_DIR from its
# OWN environment, which cannot be set for an already-running plugin, so the tour
# has to use the shared default. Records left by real agents on this machine
# would otherwise appear as extra rows nobody staged - and deleting them would
# break those agents' panels, which are somebody's live session.
STASH="${TMPDIR:-/tmp}/zj-agent-mob-stash.$$"

cleanup() {
  for s in "$SESSION" "$API_SESSION" "$INFRA_SESSION"; do
    zellij kill-session "$s" >/dev/null 2>&1 || true
    zellij delete-session "$s" --force >/dev/null 2>&1 || true
  done
  # The tour's own records go, the machine's come back.
  # Records AND the panel beacon the tour's own panel drops on each scan.
  rm -f "$SPOOL/$SESSION."* "$SPOOL/$API_SESSION."* "$SPOOL/$INFRA_SESSION."* \
        "$SPOOL/panel.$SESSION" "$SPOOL/panel.$API_SESSION" "$SPOOL/panel.$INFRA_SESSION" 2>/dev/null || true
  if [ -d "$STASH" ]; then
    mkdir -p "$SPOOL" 2>/dev/null || true
    find "$STASH" -maxdepth 1 -type f -exec mv {} "$SPOOL/" \; 2>/dev/null || true
    rmdir "$STASH" 2>/dev/null || true
  fi
  rm -rf /tmp/zj-tour-bin /tmp/zj-tour-ps.txt 2>/dev/null || true
}
trap cleanup EXIT

# --------------------------------------------------------------------- setup
sh "$DIR/tour-config.sh" "$CFG" >/dev/null
cleanup

if [ -d "$SPOOL" ]; then
  mkdir -p "$STASH"
  find "$SPOOL" -maxdepth 1 -type f -exec mv {} "$STASH/" \; 2>/dev/null || true
fi
sleep 1

# A `ps` that reports only the tour's own agents, so the machine's real ones
# stay out of frame. Scoped to the tour by PATH: the Zellij server this script
# starts below inherits it, nothing outside does.
STUB=/tmp/zj-tour-bin
rm -rf "$STUB"
mkdir -p "$STUB"
cat > "$STUB/ps" <<'EOS'
#!/bin/sh
# Only the tour's panes exist as far as the scan is concerned. The format matches
# `ps axeww -o pid=,command=`: pid, command, then environment words.
exec /bin/cat "$ZJ_TOUR_PSFILE"
EOS
chmod +x "$STUB/ps"
: > /tmp/zj-tour-ps.txt
export ZJ_TOUR_PSFILE=/tmp/zj-tour-ps.txt

# The session needs a pty to exist at all, and `script` is what provides one
# without a terminal emulator. Its output is discarded: nothing in this session
# is ever looked at directly, only the panel is read back.
script -q /dev/null sh -c \
  "PATH='$STUB:$PATH' ZJ_TOUR_PSFILE=/tmp/zj-tour-ps.txt ZELLIJ_CONFIG_DIR='$CFG' zellij -s '$SESSION'" \
  >/dev/null 2>&1 &
_i=0
while [ "$_i" -lt 40 ]; do
  zellij list-sessions 2>/dev/null | sed 's/\x1b\[[0-9;]*m//g' | grep -q "^$SESSION " && break
  sleep 0.25
  _i=$((_i + 1))
done
sleep 2

# The prop sessions, each given a client from its own pty. A client is required:
# the panel marks a row's session dead unless Zellij's SessionUpdate lists it,
# and SessionUpdate omits any session nothing is attached to.
for s in "$API_SESSION" "$INFRA_SESSION"; do
  zellij attach --create-background "$s" >/dev/null 2>&1 || true
done
sleep 1
for s in "$API_SESSION" "$INFRA_SESSION"; do
  script -q /dev/null sh -c "ZELLIJ_CONFIG_DIR='$CFG' zellij attach '$s'" >/dev/null 2>&1 &
done
sleep 4

A1=$(za_in "$API_SESSION" new-pane --name claude -- sh -c \
  "ZJ_MOCK_PROMPT='git push --force' sh '$MOCK' claude 'PAY-2903 checkout 500s on retry' \
   'Read services/checkout/handler.rs' 'Bash cargo test -p checkout'" 2>/dev/null \
  | tr -d '[:space:]' | sed 's/^terminal_//')
A2=$(za_in "$API_SESSION" new-pane --name codex -- sh -c \
  "sh '$MOCK' codex 'WEB-771 dark mode tokens' 'Edit apps/web/src/theme.ts'" 2>/dev/null \
  | tr -d '[:space:]' | sed 's/^terminal_//')
I1=$(za_in "$INFRA_SESSION" new-pane --name codex -- sh -c \
  "sh '$MOCK' codex 'Bump the prod node pool to 1.30' 'Edit terraform/eks/pool.tf'" 2>/dev/null \
  | tr -d '[:space:]' | sed 's/^terminal_//')

# The local fleet: Claude and Codex mixed, spread across backend, frontend,
# fullstack and data work, because one engineer's agents do not all live in one
# repo and grouping only means something once they do not.
P1=$(new_agent claude "PAY-2841 retry idempotent charges" "" \
  "Read services/payments/charge.rs" "Edit services/payments/charge.rs")
P2=$(new_agent codex "WEB-624 virtualize the orders table" "" \
  "Read apps/web/src/OrdersTable.tsx" "Bash pnpm -F web test")
P3=$(new_agent claude "Checkout flow: API + form validation" "" \
  "Edit services/checkout/handler.rs" "Edit apps/web/src/CheckoutForm.tsx")
P4=$(new_agent codex "Backfill orders_idx on replica" "rm -rf node_modules" \
  "Read migrations/0142_orders_idx.sql")

# What the stubbed `ps` reports: the tour's own agents and nothing else, in the
# `pid command ENV=...` shape the scan's awk expects. The foreign ones are listed
# too, because a foreign row has to be discovered once before the spool poll will
# refresh it - spool_poll_due() only fires when a foreign row already exists.
{
  printf '9001 claude ZELLIJ=0 ZELLIJ_PANE_ID=%s ZELLIJ_SESSION_NAME=%s\n' "$P1" "$SESSION"
  printf '9002 codex ZELLIJ=0 ZELLIJ_PANE_ID=%s ZELLIJ_SESSION_NAME=%s\n' "$P2" "$SESSION"
  printf '9003 claude ZELLIJ=0 ZELLIJ_PANE_ID=%s ZELLIJ_SESSION_NAME=%s\n' "$P3" "$SESSION"
  printf '9004 codex ZELLIJ=0 ZELLIJ_PANE_ID=%s ZELLIJ_SESSION_NAME=%s\n' "$P4" "$SESSION"
  printf '9005 claude ZELLIJ=0 ZELLIJ_PANE_ID=%s ZELLIJ_SESSION_NAME=%s\n' "$A1" "$API_SESSION"
  printf '9006 codex ZELLIJ=0 ZELLIJ_PANE_ID=%s ZELLIJ_SESSION_NAME=%s\n' "$A2" "$API_SESSION"
  printf '9007 codex ZELLIJ=0 ZELLIJ_PANE_ID=%s ZELLIJ_SESSION_NAME=%s\n' "$I1" "$INFRA_SESSION"
  # The session servers, which are what the panel reads liveness from.
  #
  # A stub that lists only agents says, accurately, that no Zellij session is
  # running anywhere - and every foreign row is then correctly marked `gone`.
  # (That is what the tour recorded before this line existed, and it looked like
  # a plugin bug because a real bug with the same symptom was sitting behind
  # it.) One line per session, in the shape the real thing has: the last
  # argument is the session's socket path, whose basename is the name.
  _sockdir=${TMPDIR:-/tmp}/zellij-$(id -u 2>/dev/null || echo 0)/contract_version_1
  printf '9101 %s --server %s/%s\n' "$(command -v zellij)" "$_sockdir" "$SESSION"
  printf '9102 %s --server %s/%s\n' "$(command -v zellij)" "$_sockdir" "$API_SESSION"
  printf '9103 %s --server %s/%s\n' "$(command -v zellij)" "$_sockdir" "$INFRA_SESSION"
} > /tmp/zj-tour-ps.txt

# The foreign agents' spool records, written up front: a discovered row with no
# record to merge shows a bare pane id instead of a task.
#
# `$1` is the Claude agent's state, so the tour can open on a fleet that is
# merely busy and let it block on camera at act 10.
spool_foreign() {
  case ${1:-blocked} in
    working)
      spool "$API_SESSION" "$A1" "tool=claude,status=working,session_id=s-a1,cwd=$REPO/checkout,task=PAY-2903 checkout 500s on retry,detail=Read services/checkout/handler.rs"
      ;;
    *)
      spool "$API_SESSION" "$A1" "tool=claude,status=waiting,session_id=s-a1,cwd=$REPO/checkout,task=PAY-2903 checkout 500s on retry,detail=needs approval: Bash git push --force,block=tool"
      ;;
  esac
  spool "$API_SESSION" "$A2" "tool=codex,status=working,session_id=s-a2,cwd=$REPO/web,task=WEB-771 dark mode tokens,detail=Edit apps/web/src/theme.ts"
  spool "$INFRA_SESSION" "$I1" "tool=codex,status=working,session_id=s-i1,cwd=$REPO/platform-infra,task=Bump the prod node pool to 1.30,detail=Edit terraform/eks/pool.tf"
}
spool_foreign working

# Compile the wasm before any frame is captured. Zellij caches compiled plugins
# per session, so a fresh session is always a cold cache and the first pipe
# blocks on a full compile.
za launch-or-focus-plugin --floating --configuration "$PLUGIN_CONF" "file:$WASM" >/dev/null 2>&1 || true
_i=0
while [ "$_i" -lt 240 ]; do
  rm -f /tmp/zj-tour-warm.txt
  za dump-screen --path /tmp/zj-tour-warm.txt >/dev/null 2>&1 || true
  if [ -s /tmp/zj-tour-warm.txt ] && grep -q 'zj-agent-mob' /tmp/zj-tour-warm.txt 2>/dev/null; then
    break
  fi
  # Grant the plugin permission if this build has none. Zellij keys it by
  # plugin path in the cache dir, so `reinstall-local.sh` drops it and the
  # panel comes up asking instead of rendering.
  if [ -s /tmp/zj-tour-warm.txt ] && grep -q 'Allow?' /tmp/zj-tour-warm.txt 2>/dev/null; then
    # The prompt only reads keys with focus, and its pane is named by the wasm
    # path rather than "Agent Mob" until the grant goes through.
    _pp=$(za list-panes 2>/dev/null | awk '/zj-agent-mob\.wasm/ { print $1; exit }' | sed 's/^plugin_//')
    [ -n "$_pp" ] && za focus-pane-id "$_pp" >/dev/null 2>&1
    za write-chars "y" >/dev/null 2>&1 || true
  fi
  sleep 0.5
  _i=$((_i + 1))
done

PANEL=$(za list-panes 2>/dev/null | awk '/Agent Mob/ { print $1; exit }')
[ -n "$PANEL" ] || { echo "no panel pane" >&2; exit 1; }

# Size to the content: the panel renders from the top, so height past the last
# row is dead space. A resize issued before the viewport settles is silently
# ignored, so settle first and let fill() re-assert it per frame.
sleep 2
fill
sleep 1.5

# ------------------------------------------------------------------ act 0
emit "pane_id=$P1,tool=claude,status=working,task=PAY-2841 retry idempotent charges,cwd=$REPO/payments,detail=Edit services/payments/charge.rs,model=opus"
emit "pane_id=$P2,tool=codex,status=working,task=WEB-624 virtualize the orders table,cwd=$REPO/web,detail=Bash pnpm -F web test"
emit "pane_id=$P3,tool=claude,status=idle,task=Checkout flow: API + form validation,cwd=$REPO/checkout,detail=Edit apps/web/src/CheckoutForm.tsx"
emit "pane_id=$P4,tool=codex,status=working,task=Backfill orders_idx on replica,cwd=$REPO/orders,detail=Read migrations/0142_orders_idx.sql"
wait_for "PAY-2841" || exit 1
snap 12

# ------------------------------------------------------------------ act 1
# An agent blocked on you is invisible until you cycle past its pane; urgency
# sorting floats it to the top.
emit "pane_id=$P2,tool=codex,status=compact,task=WEB-624 virtualize the orders table,cwd=$REPO/web"
sleep 1
snap 6
emit "pane_id=$P4,tool=codex,status=waiting,block=tool,task=Backfill orders_idx on replica,cwd=$REPO/orders,detail=needs approval: Bash psql -f migrations/0142_orders_idx.sql"
wait_for "waiting" || exit 1
snap 12

# ------------------------------------------------------------------ act 2
# The permission prompt, parked by the hook and answered from the panel.
emit_ask "pane_id=$P4,session=$SESSION,verdict_file=/tmp/zj-tour-verdict-$P4,tool_name=Bash,tool_arg=psql -f migrations/0142_orders_idx.sql,timeout=300"
wait_for "psql" || exit 1
snap 16
key a 0.6 10
emit "pane_id=$P4,tool=codex,status=working,task=Backfill orders_idx on replica,cwd=$REPO/orders,detail=Bash psql -f migrations/0142_orders_idx.sql"
sleep 1
snap 8

# `A` is the same approval plus a standing rule for that tool, so the next
# `cargo test` does not stop the turn again. Uppercase on purpose: approving
# every future call of a tool must not be one slipped finger from approving this
# one.
emit "pane_id=$P1,tool=claude,status=waiting,block=tool,task=PAY-2841 retry idempotent charges,cwd=$REPO/payments,detail=needs approval: Bash cargo test -p payments"
emit_ask "pane_id=$P1,session=$SESSION,verdict_file=/tmp/zj-tour-verdict-$P1,tool_name=Bash,tool_arg=cargo test -p payments,timeout=300"
wait_for "cargo test" || echo "act 2c: ask never rendered" >&2
snap 12
key A 0.6 12
emit "pane_id=$P1,tool=claude,status=working,task=PAY-2841 retry idempotent charges,cwd=$REPO/payments,detail=Bash cargo test -p payments,model=opus"
sleep 1
snap 6

# ------------------------------------------------------------------ act 3
# Fuzzy find: a few characters of a task narrows the list.
key / 0.5 4
type_str "orders"
snap 14
key Esc 0.5 4

# ------------------------------------------------------------------ act 4
# Answer a blocked agent in place.
emit "pane_id=$P3,tool=claude,status=waiting,block=question,task=Checkout flow: API + form validation,cwd=$REPO/checkout,detail=Validate the promo code client side too?"
wait_for "promo code" || exit 1
snap 12
key m 0.5 4
type_str "yes both"
snap 10
key Enter 0.6 6
emit "pane_id=$P3,tool=claude,status=working,task=Checkout flow: API + form validation,cwd=$REPO/checkout,detail=Edit apps/web/src/CheckoutForm.tsx"
sleep 1
snap 8

# ------------------------------------------------------------------ act 5
# Queue a follow-up, delivered when the current turn ends.
key f 0.5 4
type_str "run pnpm lint"
snap 10
key Enter 0.6 8

# ------------------------------------------------------------------ act 6
# done, and dismissing the badge.
emit "pane_id=$P2,tool=codex,status=done,task=WEB-624 virtualize the orders table,cwd=$REPO/web,detail=9 files changed, 128 tests pass"
wait_for "done" || exit 1
snap 12
key j 0.4 3
key d 0.5 8

# ------------------------------------------------------------------ act 7
emit "pane_id=$P1,tool=claude,status=failed,task=PAY-2841 retry idempotent charges,cwd=$REPO/payments,detail=rate limit reached, retry after 60s"
wait_for "failed" || exit 1
snap 12

# ------------------------------------------------------------------ act 8
# Two-step kill: the first x interrupts and arms, so it cannot happen by slip.
key g 0.3 2
key 1 0.3 2
key Enter 0.5 3
key x 0.6 14
key Esc 0.5 6

# ------------------------------------------------------------------ act 9
# The install screen: Claude Code and Codex hooks toggle independently.
key i 0.8 16
key i 0.6 4

# ------------------------------------------------------------------ act 10
# The beat: an agent two sessions away blocks and sorts to the top of a panel
# you were already looking at. Re-stamped so the ages read as current here.
spool_foreign blocked
# Nudge a scan rather than waiting for one.
#
# The scan is what reads the spool, and it is driven by pane and session events -
# it does NOT run on a schedule. (The 5s spool poll only fires once a foreign row
# already exists, so it cannot bootstrap the first one.) Opening and closing a
# pane is a pane event, which is enough to make the panel look.
_nudge=$(za new-pane -- sh -c 'sleep 1' 2>/dev/null | tr -d '[:space:]')
sleep 1.5
za close-pane --pane-id "$_nudge" >/dev/null 2>&1 || true
# Not fatal: the rows arrive via the scan, whose timing is driven by pane events
# rather than by anything this script can force, and the tour is still correct
# without the last beat if it never lands.
wait_for "PAY-2903" || echo "act 10: foreign rows never rendered" >&2
snap 16

# ------------------------------------------------------------------ act 11
# Grouped by session: one panel, three sessions, both tools.
key s 0.7 12
key s 0.7 20

# ------------------------------------------------------------------ act 12
# The payoff: Enter on a row in ANOTHER session lands in that agent's pane.
# Selected by find rather than row number, whose position depends on the
# grouping act 11 left switched on.
key / 0.5 4
type_str "2903"
snap 10
key Enter 0.7 4

# The jump moves the client, so this frame is read from the other session.
# `dump-screen` reads whichever pane has focus and the jump's focus change is
# async, so focus A1 explicitly and wait for ITS content, not any content.
sleep 2
za_in "$API_SESSION" focus-pane-id "$A1" >/dev/null 2>&1 || true
sleep 1
_i=0
while [ "$_i" -lt 24 ]; do
  rm -f /tmp/zj-tour-jump.txt
  za_in "$API_SESSION" focus-pane-id "$A1" >/dev/null 2>&1 || true
  za_in "$API_SESSION" dump-screen --path /tmp/zj-tour-jump.txt >/dev/null 2>&1 || true
  if [ -s /tmp/zj-tour-jump.txt ] && grep -q 'force' /tmp/zj-tour-jump.txt 2>/dev/null; then
    break
  fi
  sleep 0.5
  _i=$((_i + 1))
done
if [ -s /tmp/zj-tour-jump.txt ] && grep -q 'force' /tmp/zj-tour-jump.txt 2>/dev/null; then
  SEQ=$((SEQ + 1))
  _f=$(printf '%s/%04d' "$FRAMES" "$SEQ")
  cp /tmp/zj-tour-jump.txt "$_f.txt"
  echo 24 > "$_f.hold"
else
  echo "act 12: never read the destination pane" >&2
fi

echo "captured $SEQ frames to $FRAMES"
