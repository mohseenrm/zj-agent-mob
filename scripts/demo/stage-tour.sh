#!/bin/sh
# Drives the full-feature tour. Run from outside the recorded session; the tape
# only starts Zellij and waits.
#
#   ZJ_SESSION=demo sh scripts/demo/stage-tour.sh
#
# Every status here is synthetic, piped in over the same `zellij pipe` interface
# the real hook uses, so the panel cannot tell this from a live agent. Pane ids
# are real, because reconcile() culls any agent whose pane is gone.
set -e

: "${ZJ_SESSION:?set ZJ_SESSION}"
# shellcheck source=scripts/demo/lib.sh
. "$(dirname "$0")/lib.sh"

LOG=${ZJ_DEMO_LOG:-/dev/null}
act() { echo "act: $1" >>"$LOG"; }

# Four panes: three agents plus one left idle so the list has a quiet row.
open_panes 4
# shellcheck disable=SC2046 # word splitting is the point: one arg per pane id
set -- $(pane_ids)
P1=$1 P2=$2 P3=$3 P4=$4

# ---------------------------------------------------------------- act 1: appear
act "1 appear"
# One agent starts a turn. This first emit also launches the panel.
emit "pane_id=$P1,tool=claude,status=working,task=Refactor the auth middleware,cwd=~/repo/web,detail=Edit src/auth/middleware.rs"
show_panel

wait_for "claude" || exit 1
sleep 1.5

# A second agent joins, mid-turn, with a tool call on its detail line.
emit "pane_id=$P2,tool=codex,status=working,task=Fix flaky checkout test,cwd=~/repo/api,detail=Bash cargo test --lib"
sleep 1.8

# A third, running unattended: the perm_mode badge is the tell.
emit "pane_id=$P3,tool=claude,status=working,task=Add retry to webhook sender,cwd=~/repo/jobs,detail=Read src/webhook.rs,perm_mode=bypassPermissions"
sleep 2

# ------------------------------------------------- act 2: fan-out and progress
act "2 fanout"
# Subagent + task counters arrive as deltas on an existing row (empty status).
emit "pane_id=$P1,status=,subagent_delta=1,agent_type=explore"
emit "pane_id=$P1,status=,subagent_delta=1,agent_type=explore"
emit "pane_id=$P1,status=,task_delta=7"
sleep 1.5
emit "pane_id=$P1,status=,task_done_delta=1"
emit "pane_id=$P1,status=,task_done_delta=1"
emit "pane_id=$P1,status=,task_done_delta=1"
emit "pane_id=$P1,status=,task_done_delta=1"
sleep 2

# ------------------------------------------------ act 3: something needs you
act "3 waiting"
# `waiting` outranks `working`, so this row jumps to the top on its own.
emit "pane_id=$P2,tool=codex,status=waiting,task=Fix flaky checkout test,cwd=~/repo/api,detail=needs approval: rm -rf node_modules"
wait_for "waiting" || exit 1
sleep 2.5

# Park the actual prompt so the approve/reject box renders under the row.
emit_ask "pane_id=$P2,verdict_file=/tmp/zj-demo-verdict,tool_name=Bash,tool_arg=rm -rf node_modules"
sleep 3

# Answer it from the panel: `a` approves without leaving the list.
key "a" 2.5

# ----------------------------------------------------- act 4: the other states
act "4 states"
# compact looks like a hang unless it is labelled.
emit "pane_id=$P3,tool=claude,status=compact,task=Add retry to webhook sender,cwd=~/repo/jobs,detail=compacting context (auto)"
sleep 2.5

# failed: rate limit / billing / auth.
emit "pane_id=$P3,tool=claude,status=failed,task=Add retry to webhook sender,cwd=~/repo/jobs,detail=rate limit reached, retry in 60s"
sleep 2.5

# done carries the turn's closing message rather than a stale summary.
emit "pane_id=$P1,tool=claude,status=done,task=Refactor the auth middleware,cwd=~/repo/web,detail=Extracted the token check into a tower layer and updated 3 call sites"
sleep 2.5

# An idle agent, for contrast: session open, nothing new.
emit "pane_id=$P4,tool=codex,status=idle,task=Review the release checklist,cwd=~/repo/infra"
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

# Land on a full, calm list for the final frames.
emit "pane_id=$P2,tool=codex,status=working,task=Fix flaky checkout test,cwd=~/repo/api,detail=Bash cargo test --lib"
sleep 3
act "done"
