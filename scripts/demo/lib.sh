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

# Send one agent-status message to the plugin. `pipe --plugin` auto-launches the
# plugin, so the first call is also what puts the panel on screen.
#
# The trailing `|| true` matters: `zellij pipe` can exit nonzero even when the
# message was delivered, and callers run under `set -e`. Without it the tour dies
# mid-act with an empty stderr, which looks exactly like a hang.
emit() {
  za pipe --name agent-status --plugin "file:$WASM" \
    --plugin-configuration "$PLUGIN_CONF" --args "$1" >/dev/null 2>&1 || true
}

# Park a permission prompt in the panel (the `a`/`r` approve-reject box).
emit_ask() {
  za pipe --name agent-ask --plugin "file:$WASM" \
    --plugin-configuration "$PLUGIN_CONF" --args "$1" >/dev/null 2>&1 || true
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
spawn_session() {
  _name=$1
  zellij delete-session "$_name" --force >/dev/null 2>&1 || true
  za new-pane -d down --name "$_name" -- sh -c "zellij attach --create $_name" >/dev/null 2>&1 || true
  _i=0
  while [ "$_i" -lt 60 ]; do
    if zellij list-sessions 2>/dev/null | sed 's/\x1b\[[0-9;]*m//g' | grep -q "^$_name "; then
      sleep 0.8
      # Every fresh session opens the "About Zellij / Zellij Tip" modal, which
      # sits squarely over the pane this session exists to show. It is a
      # FLOATING plugin pane: `close-pane` does not remove it (verified) and an
      # Esc has to reach the right client, but toggling floating panes hides it
      # outright.
      za_in "$_name" toggle-floating-panes >/dev/null 2>&1 || true
      sleep 0.4
      # `zellij attach` runs INSIDE a pane of the recorded session, so focus is
      # now in the nested session. Everything after this drives the outer one.
      za focus-next-pane >/dev/null 2>&1 || true
      sleep 0.3
      return 0
    fi
    sleep 0.25
    _i=$((_i + 1))
  done
  echo "spawn_session: $_name never appeared" >&2
  return 1
}

# Run a mock agent transcript in a pane of another session.
#
#   mock_in <session> <tool> <task> [prompt] [step...]
#
# `prompt` non-empty leaves the pane on a permission prompt, which is what the
# panel's `waiting` row means and what the cross-session jump lands on.
mock_in() {
  _sess=$1 _tool=$2 _task=$3 _prompt=$4
  shift 4
  _args=""
  for _s in "$@"; do
    _args="$_args '$_s'"
  done
  za_in "$_sess" new-pane --stacked --name "$_tool" -- sh -c \
    "ZJ_MOCK_PROMPT='$_prompt' sh '$MOCK' '$_tool' '$_task'$_args" >/dev/null 2>&1 || true
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
# `--stacked` keeps them as one collapsed stack instead of tiling the viewport
# into ever-smaller slivers, which is also how a real multi-agent session is
# usually arranged.
open_agent() {
  _tool=$1 _task=$2 _prompt=$3
  shift 3
  _args=""
  for _s in "$@"; do
    _args="$_args '$_s'"
  done
  [ -f "$MOCK" ] || echo "open_agent: MOCK missing at $MOCK" >&2
  za new-pane --stacked --name "$_tool" -- sh -c \
    "ZJ_MOCK_PROMPT='$_prompt' sh '$MOCK' '$_tool' '$_task'$_args" >/dev/null 2>&1 || true
  sleep 0.8
}

# Bring the panel up, size it, and leave it focused so `send-keys` reaches the
# plugin rather than whichever pane was created last.
#
# Two reasons to set the geometry explicitly: the default floating size is a
# fraction of the viewport, which truncates the task column mid-word; and the
# demo is about the panel, so tiled shell prompts behind it are clutter rather
# than context. The panes stay alive (reconcile() needs them), just out of shot.
show_panel() {
  PANEL_PANE=$(za launch-or-focus-plugin --floating --move-to-focused-tab \
    --configuration "$PLUGIN_CONF" "file:$WASM" 2>/dev/null | tr -d '[:space:]')
  sleep 1.5
  [ -n "$PANEL_PANE" ] || PANEL_PANE=$(za list-panes 2>/dev/null | awk '/zj-agent-mob\.wasm/ { print $1; exit }')
  [ -n "$PANEL_PANE" ] || return 1
  fill_frame
  sleep 1
}

# Re-assert the panel geometry. Switching between the list and the install screen
# can snap the floating pane back to its default size, which puts the prop panes
# back in frame - so call this after any view toggle, not just at startup.
fill_frame() {
  [ -n "$PANEL_PANE" ] || return 0
  za change-floating-pane-coordinates --pane-id "$PANEL_PANE" \
    --x 2% --y 3% --width 96% --height 90% >/dev/null 2>&1 || true
}

# Poll until the panel has rendered `$1`, instead of guessing with sleep.
# Returns 1 on timeout so a broken demo fails loudly rather than recording junk.
#
# Match on short, stable strings: the task column truncates with an ellipsis
# when the pane is narrow, so a long needle can never match.
wait_for() {
  needle=$1
  tries=${2:-40}
  i=0
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
