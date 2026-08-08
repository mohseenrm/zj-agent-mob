#!/bin/sh
# Shared helpers for driving a Zellij session from OUTSIDE it.
#
# Why not keystrokes: Zellij's input is modal, so a tape that types `Ctrl+S` then
# `c` depends on which mode the session happens to be in. `zellij action` is
# unconditional and returns an exit code. See docs/demo.md.
#
# Requires: ZJ_SESSION set by the caller.

WASM="$HOME/.config/zellij/plugins/zj-agent-mob.wasm"
# `$0` inside a sourced file is the CALLER, not this file, so `dirname "$0"`
# silently resolves to wherever the caller lives. Callers set ZJ_DEMO_DIR; the
# fallback keeps a hand-run `sh scripts/demo/lib.sh` working.
MOCK="${ZJ_DEMO_DIR:-scripts/demo}/mock-agent.sh"
# Bar-free layout, used by the recorded session and every session the tour hops
# into, so the frame keeps the same shape throughout.
# Hard cap for `zellij pipe`, which blocks waiting for a plugin reply.
#
# 8s, not 2s: `pipe --plugin` AUTO-LAUNCHES the plugin, and the first call of the
# tour therefore waits for Zellij to compile the wasm. A 2s cap killed that first
# emit, so no agent ever registered and the panel sat on "no agents in this
# session" until `wait_for` gave up. The cap only has to be shorter than the
# stall it prevents (tens of seconds), not short in absolute terms.
#
# GNU coreutils installs `gtimeout` on macOS; Homebrew also provides `timeout`.
#
# ZJ_TIMEOUT holds the BINARY NAME only, and it is applied to `zellij` itself in
# `za_pipe` below - never to a shell function. `timeout` is an external command
# and execs its argument, so it cannot run a function: `$ZJ_TIMEOUT za pipe ...`
# failed with `timeout: failed to run command 'za'` and exit 127, in every shell.
# Because `emit` ends in `2>&1 || true` that was completely silent, and every
# status the tour piped was a no-op - the panel sat on "no agents in this
# session" while each act reported success.
if command -v timeout >/dev/null 2>&1; then
  ZJ_TIMEOUT=timeout
elif command -v gtimeout >/dev/null 2>&1; then
  ZJ_TIMEOUT=gtimeout
else
  ZJ_TIMEOUT=""
fi
ZJ_TIMEOUT_SECS=${ZJ_TIMEOUT_SECS:-8}

# Floating panel geometry, as percentages of the viewport. Tuned so the panel is
# wide enough not to truncate the task column and tall enough for the longest
# view (the install screen), without leaving a dead band under the last row.
# Y is set so the panel sits centred rather than pinned to the top. The panel
# renders content from the top of its pane and the tour's tallest view is about
# 14 rows, so the pane is sized close to that and pushed down by roughly half the
# slack: at 6% the frame had a large dead band underneath it.
PANEL_X=${PANEL_X:-4%}
PANEL_Y=${PANEL_Y:-22%}
PANEL_W=${PANEL_W:-92%}
PANEL_H=${PANEL_H:-52%}

# `discover false` turns off the process scan for the recording only: without it
# the panel also lists the real agents running on the recording machine, which is
# correct behaviour and wrong for a demo.
#
# Zellij treats "same url, different configuration" as a DIFFERENT plugin, for
# both launching and pipe routing. So this string has to be passed identically to
# every launch AND every pipe, or the tour ends up with two Agent Mob panes: one
# holding the rows and one that was launched with no config.
PLUGIN_CONF="discover=false"

# `zellij action` with stdin closed. Without </dev/null, `pipe` with no PAYLOAD
# argument listens on stdin and blocks forever.
za() {
  zellij -s "$ZJ_SESSION" action "$@" </dev/null
}

# Same, against a named session. The cross-session acts drive two other
# sessions without moving the recorded session's focus.
za_in() {
  _s=$1
  shift
  zellij -s "$_s" action "$@" </dev/null
}

# `zellij action pipe`, time-capped. The cap wraps the `zellij` BINARY, which is
# the whole reason this exists as its own helper rather than as `$ZJ_TIMEOUT za`:
# `timeout` execs its argument, so handing it the `za` function is an instant
# exit 127 and a silently undelivered message.
za_pipe() {
  if [ -n "$ZJ_TIMEOUT" ]; then
    $ZJ_TIMEOUT "$ZJ_TIMEOUT_SECS" zellij -s "$ZJ_SESSION" action pipe "$@" </dev/null
  else
    zellij -s "$ZJ_SESSION" action pipe "$@" </dev/null
  fi
}

# Send one agent-status message to the plugin. `pipe --plugin` auto-launches the
# plugin, so the first call is also what puts the panel on screen.
#
# The trailing `|| true` matters: `zellij pipe` can exit nonzero even when the
# message was delivered, and callers run under `set -e`. Without it the tour dies
# mid-act with an empty stderr, which looks exactly like a hang.
#
# Timeout-capped, because `zellij pipe` BLOCKS waiting for the plugin to reply.
# A panel that is on screen answers at once, so this never looked like a problem
# - but act 3b presses `q`, and against a hidden panel every emit stalled for
# tens of seconds. The tour went from 99s to 284s with an empty error log,
# because each call did eventually succeed.
#
# Backgrounding alone is NOT enough: the calls simply pile up and the next
# `zellij action` queues behind them (measured worse - one act at +1086s). The
# cap has to be hard. The message is delivered well inside the timeout; what is
# being abandoned is only the wait for the reply.
#
# Exit 124 is `timeout` firing, which is expected and harmless - the message was
# delivered, only the reply went unheard. Any OTHER nonzero status is reported,
# because the failure this replaces (exit 127, `timeout` handed a shell function)
# was invisible for four renders behind a blanket `2>&1 || true`.
emit() {
  _e=0
  _out=$(za_pipe --name agent-status --plugin "file:$WASM" \
    --plugin-configuration "$PLUGIN_CONF" --args "$1" 2>&1) || _e=$?
  case $_e in
    0 | 124) ;;
    *) echo "emit: exit $_e for '${1%%,*}...': $_out" >&2 ;;
  esac
}

# Park a permission prompt in the panel (the `a`/`r` approve-reject box).
emit_ask() {
  _e=0
  _out=$(za_pipe --name agent-ask --plugin "file:$WASM" \
    --plugin-configuration "$PLUGIN_CONF" --args "$1" 2>&1) || _e=$?
  case $_e in
    0 | 124) ;;
    *) echo "emit_ask: exit $_e for '${1%%,*}...': $_out" >&2 ;;
  esac
}

# Create another Zellij session, attached from a pane of the recorded one.
#
# KNOWN LIMIT, verified three ways: the panel will show this session's agents as
# `unknown`, not live. `SessionUpdate` - the only way a plugin learns which
# sessions exist - does not report a session whose sole client is nested inside
# another session, which is the only kind this script can create headlessly.
# `zellij attach --create-background` is worse (no client at all), and a ttyd
# host never starts without a browser.
#
# That is honest for the demo: it is exactly what a user sees for a session that
# is not currently attached anywhere, and `Enter` on such a row attaches it.
# What it cannot show is a foreign row streaming live status.
#
# SPLIT IN TWO, and the order is load-bearing: create_session, then every
# mock_in, THEN attach_session. A `new-pane` aimed at a session whose only client
# is nested inside another session prints a pane id, exits 0 - and the pane is
# gone by the next `list-panes`, because that client is not rendering and never
# processes the new pane. Verified side by side: identical calls persist against
# a directly-attached session and vanish against a nested one, which is why the
# act-9 hop used to land on an empty pane instead of an agent transcript.
#
# Panes created BEFORE any client attaches do survive the attach, so the tour
# builds each foreign session's contents first and hands it a client afterwards.
create_session() {
  _name=$1
  zellij delete-session "$_name" --force >/dev/null 2>&1 || true
  # No client at all yet. This is deliberately the state trap #13 warns is
  # useless for `--stacked` - so mock_in must NOT pass --stacked, and does not.
  zellij attach --create-background "$_name" >/dev/null 2>&1 || true
  _i=0
  while [ "$_i" -lt 60 ]; do
    if zellij list-sessions 2>/dev/null | sed 's/\x1b\[[0-9;]*m//g' | grep -q "^$_name "; then
      sleep 0.5
      return 0
    fi
    sleep 0.25
    _i=$((_i + 1))
  done
  echo "create_session: $_name never appeared" >&2
  return 1
}

# Give a prepared session a client, from a pane of the recorded one.
#
# The client is what makes the session real to the demo: `Enter` on a foreign row
# attaches it, and act 9 lands the recorded client in it.
attach_session() {
  _name=$1
  # The pane the act-9 hop should land on: the agent this session exists to show.
  _land=${2:-}
  # The layout comes from ZELLIJ_CONFIG_DIR's `default_layout`, which the pane
  # inherits from the recorded shell. Passing `-l` here instead does NOT work:
  # with a session name also given, Zellij adds the layout as a new tab rather
  # than creating the session with it.
  za new-pane --name "$_name" -- sh -c "zellij attach $_name" >/dev/null 2>&1 || true
  sleep 1.5
  # Now that the session HAS a client, collapse its panes into one stack.
  #
  # mock_in had to create them unstacked (`--stacked` silently fails with no
  # client attached, trap #13), which leaves them tiled - and a tiled grid means
  # the pane the nested client renders into is a fraction of the viewport. After
  # the act-9 hop that viewport IS the recorded frame, so the act-10 panel came
  # out as a small box in the corner. Stacking now, with a client present, gives
  # the client a full-size pane to land in.
  fullscreen_in "$_name" "$_land"
  # Every fresh session opens the "About Zellij / Zellij Tip" modal, which sits
  # squarely over the pane this session exists to show. It is a FLOATING plugin
  # pane: `close-pane` does not remove it (verified) and an Esc has to reach the
  # right client, but toggling floating panes hides it outright.
  za_in "$_name" toggle-floating-panes >/dev/null 2>&1 || true
  sleep 0.4
  # Create this session's OWN panel now, while the session still has a client of
  # its own.
  #
  # It cannot be created later, from act 10: by then this session's only clients
  # are the nested ones, and `launch-or-focus-plugin` against such a session
  # prints a pane id and exits 0 while creating nothing - the same silent failure
  # that kills `new-pane` there. Act 10's `show_panel` then re-focuses this
  # already-existing pane, which does work.
  #
  # A plugin's state lives in the session that loaded it, so this instance starts
  # empty and act 10 seeds it - exactly what a user opening the panel in a second
  # session sees.
  za_in "$_name" launch-or-focus-plugin --floating --move-to-focused-tab \
    --configuration "$PLUGIN_CONF" "file:$WASM" >/dev/null 2>&1 || true
  sleep 1.5
  # `zellij -s` runs INSIDE a pane of the recorded session, so focus is now in
  # the nested session. Everything after this drives the outer one.
  za focus-next-pane >/dev/null 2>&1 || true
  sleep 0.3
}

# Run a mock agent transcript in a pane of another session.
#
#   mock_in <session> <tool> <task> [prompt] [step...]
#
# `prompt` non-empty leaves the pane on a permission prompt, which is what the
# panel's `waiting` row means and what the cross-session jump lands on.
#
# Sets MOCK_PANE to the id of the pane it created, for exactly the reason
# open_agent does. Hardcoding `pane_id=1,2` in the emit was correct only under
# the stock layout; the bar-free one makes the session's own shell `terminal_0`,
# so `terminal_2` does not exist in a session with two mock agents and one of the
# two rows would attach to a nonexistent pane - which reconcile() then culls.
mock_in() {
  _sess=$1 _tool=$2 _task=$3 _prompt=$4
  shift 4
  _args=""
  for _s in "$@"; do
    _args="$_args '$_s'"
  done
  # NOT `--stacked`: this runs against a session created with
  # `--create-background`, which has no client yet, and trap #13 records that
  # `--stacked` silently fails there - the pane id prints and the pane is gone a
  # moment later. A plain new-pane persists, and the act-9 hop lands on it.
  MOCK_PANE=$(za_in "$_sess" new-pane --name "$_tool" -- sh -c \
    "ZJ_MOCK_PROMPT='$_prompt' sh '$MOCK' '$_tool' '$_task'$_args" 2>/dev/null \
    | tr -d '[:space:]' | sed 's/^terminal_//')
  case $MOCK_PANE in
    '' | *[!0-9]*)
      echo "mock_in: no pane id for $_tool in $_sess (got '${MOCK_PANE:-}')" >&2
      MOCK_PANE=""
      ;;
  esac
  sleep 0.8
}

# Space-separated list of REAL terminal pane ids, ascending.
#
# Matches `^terminal_` only: a bare `grep -o '[0-9]*'` also matches the
# `plugin_0` row and silently shifts every id by one.
pane_ids() {
  za list-panes 2>/dev/null \
    | awk '/^terminal_/ { sub(/terminal_/, "", $1); print $1 }' \
    | sort -n | tr '\n' ' '
}

# Open one agent pane, stacked with the others.
#
#   open_agent <tool> <task> [prompt] [step...]
#
# The pane has to exist for reconcile() to keep its agent row, and it has to
# *look* like an agent for the moment the demo jumps into one - so each runs a
# mock transcript rather than a bare `sleep`. Never an interactive shell: it
# inherits the user's profile, which prints a banner into frame and can exit
# non-zero, taking the pane and its agent row with it.
#
# NOT `--stacked`, and this is what finally cleared the frame chrome. Zellij
# always draws a title bar per member of a stack so the stack stays navigable, so
# `pane_frames false` has no effect on stacked panes - verified in isolation, the
# same config renders `Pane #1` / `SCROLL: 0/49` stacked and nothing at all
# unstacked. The tour instead fullscreens ONE pane (see fullscreen_one), which
# hides every other pane outright and needs no frames.
# Sets AGENT_PANE to the id of the pane it created.
#
# Read from `new-pane`'s own output rather than inferred from `pane_ids()` by
# position. Position is wrong the moment the session contains any pane the tour
# did not open for an agent - the recording layout's own shell is `terminal_0`,
# and the nested-session panes land in the same list - so `set -- $(pane_ids)`
# assigned P1 to the layout shell and never named the fourth agent at all. Rows
# still appeared (those are real panes), which is why it looked like the panel
# had lost them rather than like the ids were off by one.
open_agent() {
  _tool=$1 _task=$2 _prompt=$3
  shift 3
  _args=""
  for _s in "$@"; do
    _args="$_args '$_s'"
  done
  [ -f "$MOCK" ] || echo "open_agent: MOCK missing at $MOCK" >&2
  AGENT_PANE=$(za new-pane --name "$_tool" -- sh -c \
    "ZJ_MOCK_PROMPT='$_prompt' sh '$MOCK' '$_tool' '$_task'$_args" 2>/dev/null \
    | tr -d '[:space:]' | sed 's/^terminal_//')
  case $AGENT_PANE in
    '' | *[!0-9]*)
      echo "open_agent: no pane id for $_tool (got '${AGENT_PANE:-}')" >&2
      AGENT_PANE=""
      ;;
  esac
  sleep 0.8
}

# Show exactly ONE terminal pane, full viewport, and hide every other one.
#
# This replaces the earlier stack-everything approach and is what makes the frame
# clean. `pane_frames false` does not apply to stacked panes - Zellij needs a
# title bar per stack member to keep the stack navigable - so a stacked layout
# always carried `Pane #1` / `SCROLL: 0/49` chrome no matter what the config said.
# Verified in isolation: identical config, chrome when stacked, none when not.
#
# Fullscreen sidesteps the whole question. One pane fills the viewport, the rest
# are not rendered at all, and there is no stack to label - so the recorded frame
# is the floating panel on a plain background, which is the demo's subject.
#
# The hidden panes stay alive, which is what reconcile() needs; they are simply
# not drawn.
fullscreen_one() {
  _target=${1:-}
  if [ -n "$_target" ]; then
    za focus-pane-id "terminal_$_target" >/dev/null 2>&1 || true
    sleep 0.3
  fi
  # Idempotent in the direction that matters: only turn it ON. `toggle-fullscreen`
  # against an already-fullscreen pane would put every pane back on screen.
  case $(za list-panes 2>/dev/null | grep -c 'IN FULLSCREEN') in
    0) za toggle-fullscreen >/dev/null 2>&1 || true ;;
  esac
  sleep 0.5
}

# Same, for a foreign session: fullscreen the pane the act-9 hop should land on.
#
# Called from attach_session once the session has a client of its own. Without
# it the foreign session renders its panes tiled, and after the hop that tiled
# grid is the recorded frame.
# `IN FULLSCREEN` in `list-panes` is not a reliable guard here: fullscreen is
# per-CLIENT, and after the act-9 hop the session has two clients whose views
# disagree - the one that matters is the newly-arrived one, which is tiled. So
# this asserts by toggling off first (a no-op for a client that is already tiled)
# and then on, rather than trusting the reported state.
fullscreen_in() {
  _sess=$1
  _target=${2:-}
  if [ -n "$_target" ]; then
    za_in "$_sess" focus-pane-id "terminal_$_target" >/dev/null 2>&1 || true
    sleep 0.3
  fi
  za_in "$_sess" toggle-fullscreen >/dev/null 2>&1 || true
  sleep 0.4
  # If that turned it OFF, turn it back on; if it turned it ON, this second call
  # is what would turn it off - so check what actually happened.
  case $(za_in "$_sess" list-panes 2>/dev/null | grep -c 'IN FULLSCREEN') in
    0) za_in "$_sess" toggle-fullscreen >/dev/null 2>&1 || true ;;
  esac
  sleep 0.5
}

# Bring the panel up, size it, and leave it focused so `send-keys` reaches the
# plugin rather than whichever pane was created last.
#
# Two reasons to set the geometry explicitly: the default floating size is a
# fraction of the viewport, which truncates the task column mid-word; and the
# demo is about the panel, so tiled shell prompts behind it are clutter rather
# than context. The panes stay alive (reconcile() needs them), just out of shot.
show_panel() {
  _reported=$(za launch-or-focus-plugin --floating --move-to-focused-tab \
    --configuration "$PLUGIN_CONF" "file:$WASM" 2>/dev/null | tr -d '[:space:]')
  sleep 1.5
  # Trust `list-panes`, not the returned id. Against a session whose only clients
  # are nested inside another session, `launch-or-focus-plugin` prints a pane id
  # and exits 0 while creating nothing - so act 10 was sizing and focusing a pane
  # that did not exist, and the panel never appeared after the hop. The pane that
  # IS there is the one attach_session created earlier, and it is the one to use.
  PANEL_PANE=$(za list-panes 2>/dev/null | awk '/Agent Mob/ { print $1; exit }')
  [ -n "$PANEL_PANE" ] || PANEL_PANE=$_reported
  [ -n "$PANEL_PANE" ] || return 1
  fill_frame
  sleep 1
}

# Re-assert the panel geometry. Switching between the list and the install screen
# can snap the floating pane back to its default size, which puts the prop panes
# back in frame - so call this after any view toggle, not just at startup.
#
# Height is deliberately short of the viewport rather than filling it. The panel
# renders its content from the top, so a full-height pane leaves a large empty
# region below the last row and the whole thing reads as pinned to the top-left.
# Sized close to the content and offset, it sits centred in frame.
fill_frame() {
  [ -n "$PANEL_PANE" ] || return 0
  za change-floating-pane-coordinates --pane-id "$PANEL_PANE" \
    --x "$PANEL_X" --y "$PANEL_Y" --width "$PANEL_W" --height "$PANEL_H" >/dev/null 2>&1 || true
}

# Poll until the panel has rendered `$1`, instead of guessing with sleep.
# Returns 1 on timeout so a broken demo fails loudly rather than recording junk.
#
# Match on short, stable strings: the task column truncates with an ellipsis
# when the pane is narrow, so a long needle can never match.
#
# But NOT strings the empty state also contains. Its placeholder reads "Start
# claude or codex in a pane", so `wait_for claude` matched an EMPTY panel and
# returned 0 - which is why acts 1 and 2 reported success through four renders
# while nothing had been piped, and why act 3 ("waiting", the first needle absent
# from that text) looked like the first thing to break. Guard it explicitly
# rather than relying on every future caller picking a safe needle.
wait_for() {
  needle=$1
  tries=${2:-40}
  i=0
  case $needle in
    claude | codex | agent | pane | install | hooks)
      echo "wait_for: '$needle' also appears in the empty-state text; pick a needle only a real row can produce" >&2
      return 1
      ;;
  esac
  while [ "$i" -lt "$tries" ]; do
    # A plugin pane is only dumpable while it holds focus: passing
    # `--pane-id plugin_N` writes 0 bytes, and so does dumping when a terminal
    # pane is focused. So focus the panel, then dump with no --pane-id.
    [ -n "$PANEL_PANE" ] && za focus-pane-id "$PANEL_PANE" >/dev/null 2>&1
    rm -f /tmp/zj-demo-screen.txt
    za dump-screen --path /tmp/zj-demo-screen.txt >/dev/null 2>&1
    if [ -s /tmp/zj-demo-screen.txt ] && grep -q "$needle" /tmp/zj-demo-screen.txt 2>/dev/null; then
      return 0
    fi
    sleep 0.25
    i=$((i + 1))
  done
  echo "wait_for: never saw '$needle' (panel pane: ${PANEL_PANE:-unknown})" >&2
  # Strip ANSI so the failure is readable. Literal ESC via printf, not $'..'.
  esc=$(printf '\033')
  sed -e "s/${esc}\[[0-9;?]*[a-zA-Z]//g" /tmp/zj-demo-screen.txt 2>/dev/null | head -12 >&2
  return 1
}

# Re-focus the panel. emit/new-pane and pane churn can move focus, and a
# keystroke sent to a terminal pane instead of the plugin is how a tape ends up
# typing into a shell (or opening an editor) mid-recording.
focus_panel() {
  if [ -n "$PANEL_PANE" ]; then
    za focus-pane-id "$PANEL_PANE" >/dev/null 2>&1 || true
  fi
  sleep 0.2
}

# Send a key to the panel, with a beat for the render.
#
# Re-focuses first, every time. A key sent while a terminal pane holds focus is
# silently swallowed by that pane's shell, which is how the tour used to freeze
# on whatever was last rendered: the panel kept ticking its timers but never
# received `a`, `j`, `x` or `i`. Cheap insurance - focus_pane_id on an
# already-focused pane is a no-op.
key() {
  focus_panel
  za send-keys "$1" >/dev/null 2>&1 || true
  fill_frame
  sleep "${2:-1}"
}

# Send a key WITHOUT re-asserting the geometry afterwards.
#
# For `q`: the plugin hides its own pane in response, and `fill_frame` on a pane
# the plugin just hid puts it straight back on screen - so the panel would never
# appear to close, and the auto-popup that follows would have nothing to show.
key_bare() {
  focus_panel
  za send-keys "$1" >/dev/null 2>&1 || true
  sleep "${2:-1}"
}
