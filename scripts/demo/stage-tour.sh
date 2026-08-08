#!/bin/sh
# Drives the full-feature tour. Run from outside the recorded session; the tape
# only starts Zellij and waits.
#
#   ZJ_SESSION=demo sh scripts/demo/stage-tour.sh
#
# Every status here is synthetic, piped in over the same `zellij pipe` interface
# the real hook uses, so the panel cannot tell this from a live agent. Pane ids
# are real, because reconcile() culls any agent whose pane is gone.
#
# The scenario is one engineer holding four services at once - the case the
# plugin exists for. Two of those services live in their own Zellij sessions,
# which is what acts 8-10 are about.
set -e

: "${ZJ_SESSION:?set ZJ_SESSION}"
ZJ_DEMO_DIR=$(cd "$(dirname "$0")" && pwd)
export ZJ_DEMO_DIR
# shellcheck source=scripts/demo/lib.sh
. "$ZJ_DEMO_DIR/lib.sh"

LOG=${ZJ_DEMO_LOG:-/dev/null}
act() { echo "act: $1" >>"$LOG"; }

# Four agents, each a real pane running a transcript that matches the status
# piped for it below. Stacked, so the panel is not competing with a tiled grid.
#
# These come FIRST, before the extra sessions exist. spawn_session works by
# running `zellij attach` inside a pane of this session, and focus lands inside
# that nested session - so any new-pane after it goes to the wrong session.
open_agent claude "Refactor the auth middleware" "" "Read src/auth/middleware.rs" "Edit src/auth/middleware.rs"
open_agent codex  "Fix flaky checkout test"      "" "Bash cargo test --lib"
open_agent claude "Add retry to webhook sender"  "" "Read src/webhook.rs"
open_agent codex  "Review the release checklist" ""
# shellcheck disable=SC2046 # word splitting is the point: one arg per pane id
set -- $(pane_ids)
P1=$1 P2=$2 P3=$3 P4=$4

# The other two sessions. They have to stay live for the whole tour: the panel
# flips a foreign row to `unknown` the moment Zellij stops listing its session.
API_SESSION=checkout-api
INFRA_SESSION=platform-infra
spawn_session "$API_SESSION" || true
spawn_session "$INFRA_SESSION" || true

# ---------------------------------------------------------- act 0: first open
act "0 setup"
# The panel on first open. On a machine with no hooks installed this is the
# setup screen ("Hooks are not installed", 1/2/3 to install); the recording
# machine has them installed, so it opens on the empty list instead. Act 7
# shows the install screen itself, which is the same install path and is real
# either way - faking an uninstalled machine here would mean uninstalling the
# recorder's own hooks mid-render.
show_panel
sleep 3

# ---------------------------------------------------------------- act 1: appear
act "1 appear"
# One agent starts a turn.
emit "pane_id=$P1,session=$ZJ_SESSION,tool=claude,status=working,task=Refactor the auth middleware,cwd=~/repo/web,detail=Edit src/auth/middleware.rs"

wait_for "claude" || exit 1
sleep 1.5

# A second agent joins, mid-turn, with a tool call on its detail line.
emit "pane_id=$P2,session=$ZJ_SESSION,tool=codex,status=working,task=Fix flaky checkout test,cwd=~/repo/api,detail=Bash cargo test --lib"
sleep 1.8

# A third, running unattended: the perm_mode badge is the tell.
emit "pane_id=$P3,session=$ZJ_SESSION,tool=claude,status=working,task=Add retry to webhook sender,cwd=~/repo/jobs,detail=Read src/webhook.rs,perm_mode=bypassPermissions"
sleep 2

# ------------------------------------------------- act 2: fan-out and progress
act "2 fanout"
# Subagent + task counters arrive as deltas on an existing row (empty status).
emit "pane_id=$P1,session=$ZJ_SESSION,status=,subagent_delta=1,agent_type=explore"
emit "pane_id=$P1,session=$ZJ_SESSION,status=,subagent_delta=1,agent_type=explore"
emit "pane_id=$P1,session=$ZJ_SESSION,status=,task_delta=7"
sleep 1.5
emit "pane_id=$P1,session=$ZJ_SESSION,status=,task_done_delta=1"
emit "pane_id=$P1,session=$ZJ_SESSION,status=,task_done_delta=1"
emit "pane_id=$P1,session=$ZJ_SESSION,status=,task_done_delta=1"
emit "pane_id=$P1,session=$ZJ_SESSION,status=,task_done_delta=1"
sleep 2

# ------------------------------------------------ act 3: something needs you
act "3 waiting"
# `waiting` outranks `working`, so this row jumps to the top on its own.
emit "pane_id=$P2,session=$ZJ_SESSION,tool=codex,status=waiting,task=Fix flaky checkout test,cwd=~/repo/api,detail=needs approval: rm -rf node_modules"
wait_for "waiting" || exit 1
sleep 2.5

# Park the actual prompt so the approve/reject box renders under the row.
emit_ask "pane_id=$P2,session=$ZJ_SESSION,verdict_file=/tmp/zj-demo-verdict,tool_name=Bash,tool_arg=rm -rf node_modules"
sleep 3

# Answer it from the panel: `a` approves without leaving the list.
key "a" 2.5

# ----------------------------------------------------- act 4: the other states
act "4 states"
# compact looks like a hang unless it is labelled.
emit "pane_id=$P3,session=$ZJ_SESSION,tool=claude,status=compact,task=Add retry to webhook sender,cwd=~/repo/jobs,detail=compacting context (auto)"
sleep 2.5

# failed: rate limit / billing / auth.
emit "pane_id=$P3,session=$ZJ_SESSION,tool=claude,status=failed,task=Add retry to webhook sender,cwd=~/repo/jobs,detail=rate limit reached, retry in 60s"
sleep 2.5

# done carries the turn's closing message rather than a stale summary.
emit "pane_id=$P1,session=$ZJ_SESSION,tool=claude,status=done,task=Refactor the auth middleware,cwd=~/repo/web,detail=Extracted the token check into a tower layer and updated 3 call sites"
sleep 2.5

# An idle agent, for contrast: session open, nothing new.
emit "pane_id=$P4,session=$ZJ_SESSION,tool=codex,status=idle,task=Review the release checklist,cwd=~/repo/infra"
sleep 2.5

# ------------------------------------------------------- act 5: moving around
act "5 nav"
key "j" 1.2          # move selection
key "j" 1.2
key "k" 1.5          # and back

# ---------------------------------------------------------- act 6: kill, armed
act "6 kill"
# `x` interrupts and arms the row; the red confirm makes the destructive step
# deliberate. Left armed on purpose, then backed out of.
key "x" 2.5
key "Esc" 1.5

# ------------------------------------------------------- act 7: install screen
act "7 install"
key "i" 3
key "i" 2            # back to the list

# --------------------------------------------- act 8: agents in other sessions
act "8 cross-session"
# Two more Zellij sessions, each with its own agents. Note the pane ids: every
# session hands out 0, 1, 2 - so `pane 1` is ambiguous across three sessions,
# which is why an agent is keyed by (session, pane).
mock_in "$API_SESSION" claude "Backfill idempotency keys" "" "Edit src/payments/idempotency.rs"
mock_in "$API_SESSION" codex "Shard the orders table" "" "Bash psql -f migrations/0042.sql"
mock_in "$INFRA_SESSION" claude "Roll out the new ingress" "terraform apply -auto-approve" "Bash terraform plan"

focus_panel
emit "pane_id=1,session=$API_SESSION,tool=claude,status=working,task=Backfill idempotency keys,cwd=~/repo/checkout-api,detail=Edit src/payments/idempotency.rs"
sleep 2.5
emit "pane_id=2,session=$API_SESSION,tool=codex,status=working,task=Shard the orders table,cwd=~/repo/checkout-api,detail=Bash psql -f migrations/0042.sql"
sleep 2.5
emit "pane_id=1,session=$INFRA_SESSION,tool=claude,status=waiting,task=Roll out the new ingress,cwd=~/repo/platform-infra,detail=needs approval: terraform apply -auto-approve"
wait_for "platform-" 20 || true
sleep 4

# ------------------------------------------------ act 9: jump to another session
act "9 hop"
# Select a foreign row and press Enter. The panel switches this client into that
# session and lands on the agent's own pane.
#
# The foreign rows read `unknown` here rather than streaming live status. That
# is real, not a mock: a plugin only learns which sessions exist from
# `SessionUpdate`, which does not report a session whose only client is nested
# inside another one - the only kind this script can create headlessly. A user
# running these sessions in their own terminals sees live status instead.
focus_panel
# Re-dump now, after the foreign rows have landed: the file `wait_for` left
# behind predates them. Then walk from the cursor to the first foreign row.
rm -f /tmp/zj-demo-screen.txt
za dump-screen --path /tmp/zj-demo-screen.txt >/dev/null 2>&1
_plain=$(sed -e "s/$(printf '\033')\[[0-9;?]*[a-zA-Z]//g" /tmp/zj-demo-screen.txt 2>/dev/null)
# Rows are `  N icon tool status ...`; the cursor row starts with the marker.
_cur=$(printf '%s\n' "$_plain" | grep -nE '^.?. [0-9]+ ' | grep -n '▶' | head -1 | cut -d: -f1)
_tgt=$(printf '%s\n' "$_plain" | grep -nE '^.?. [0-9]+ ' | grep -nE 'checkout-|platform-' | head -1 | cut -d: -f1)
if [ -n "$_cur" ] && [ -n "$_tgt" ] && [ "$_tgt" -gt "$_cur" ]; then
  _n=$((_tgt - _cur))
  echo "act: walking $_n rows to the foreign one" >>"$LOG"
  while [ "$_n" -gt 0 ]; do
    key "j" 0.9
    _n=$((_n - 1))
  done
else
  echo "act: WARN foreign row not found (cur=${_cur:-?} tgt=${_tgt:-?})" >>"$LOG"
fi
sleep 1.5
key "Enter" 6

# The tip modal is created lazily, when a client first attaches - which is the
# jump itself, so hiding it during spawn_session was too early. Hide it in both
# candidate targets; whichever we landed in is the one that matters.
for _s in "$API_SESSION" "$INFRA_SESSION"; do
  za_in "$_s" toggle-floating-panes >/dev/null 2>&1 || true
done
sleep 2.5

# --------------------------------- act 10: the same panel, from the new session
act "10 back"
# NOT `list-sessions | grep (current)`: that reports the session this staging
# script runs from, which never changed. Ask each candidate which one now holds
# the recorded client - the jump moved it there.
LANDED=""
for _s in "$API_SESSION" "$INFRA_SESSION"; do
  if za_in "$_s" list-clients 2>/dev/null | grep -qE '^[0-9]'; then
    LANDED=$_s
    break
  fi
done
[ -n "$LANDED" ] || LANDED=$API_SESSION
echo "act: landed in $LANDED" >>"$LOG"

# Re-seed the rows here. This is a different plugin instance - a plugin's state
# lives in the session that loaded it - so it starts empty, exactly as it would
# for a user opening the panel in a session for the first time. The hooks in
# each session are what fill it in real use; here the same pipes do.
ZJ_SESSION=$LANDED emit "pane_id=1,session=$LANDED,tool=claude,status=working,task=Backfill idempotency keys,cwd=~/repo/checkout-api,detail=Edit src/payments/idempotency.rs"
ZJ_SESSION=$LANDED emit "pane_id=2,session=$LANDED,tool=codex,status=working,task=Shard the orders table,cwd=~/repo/checkout-api,detail=Bash psql -f migrations/0042.sql"
ZJ_SESSION=$LANDED emit "pane_id=1,session=$INFRA_SESSION,tool=claude,status=waiting,task=Roll out the new ingress,cwd=~/repo/platform-infra,detail=needs approval: terraform apply -auto-approve"
# The row for the session we came from. Spelled out rather than "$ZJ_SESSION",
# which inside a `VAR=x cmd` argument still expands to the OLD value.
DEMO_SESSION=$ZJ_SESSION
ZJ_SESSION=$LANDED emit "pane_id=9,session=$DEMO_SESSION,tool=claude,status=done,task=Refactor the auth middleware,cwd=~/repo/web,detail=Extracted the token check into a tower layer"
sleep 1
ZJ_SESSION=$LANDED show_panel || true
sleep 5
act "done"
