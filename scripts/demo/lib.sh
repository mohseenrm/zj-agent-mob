#!/bin/sh
# Shared helpers for driving a Zellij session from OUTSIDE it.
#
# Why not keystrokes: Zellij's input is modal, so a tape that types `Ctrl+S` then
# `c` depends on which mode the session happens to be in. `zellij action` is
# unconditional and returns an exit code. See docs/demo.md.
#
# Requires: ZJ_SESSION set by the caller.

WASM="$HOME/.config/zellij/plugins/zj-agent-mob.wasm"

# `zellij action` with stdin closed. Without </dev/null, `pipe` with no PAYLOAD
# argument listens on stdin and blocks forever.
za() {
  zellij -s "$ZJ_SESSION" action "$@" </dev/null
}

# Send one agent-status message to the plugin. `pipe --plugin` auto-launches the
# plugin, so the first call is also what puts the panel on screen.
#
# The trailing `|| true` matters: `zellij pipe` can exit nonzero even when the
# message was delivered, and callers run under `set -e`. Without it the tour dies
# mid-act with an empty stderr, which looks exactly like a hang.
emit() {
  za pipe --name agent-status --plugin "file:$WASM" --args "$1" >/dev/null 2>&1 || true
}

# Park a permission prompt in the panel (the `a`/`r` approve-reject box).
emit_ask() {
  za pipe --name agent-ask --plugin "file:$WASM" --args "$1" >/dev/null 2>&1 || true
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

# Open n terminal panes that stay alive.
#
# The pane only has to exist: reconcile() culls any agent whose pane is gone, and
# nothing reads what runs inside. So run an explicit `sleep` rather than a shell.
# An interactive shell here is a liability - it inherits the user's profile, which
# both prints a banner into frame and can exit non-zero, taking the pane (and the
# agent row with it) down a second after it appears.
open_panes() {
  n=$1
  i=0
  while [ "$i" -lt "$n" ]; do
    za new-pane -d down -- sh -c 'sleep 100000' >/dev/null 2>&1
    sleep 0.5
    # The pane title defaults to the command, and `sh -c sleep 100000` in frame
    # gives away that these are props. Name them after what they stand in for.
    za rename-pane "agent $((i + 1))" >/dev/null 2>&1 || true
    sleep 0.2
    i=$((i + 1))
  done
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
  PANEL_PANE=$(za launch-or-focus-plugin --floating --move-to-focused-tab "file:$WASM" 2>/dev/null | tr -d '[:space:]')
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
