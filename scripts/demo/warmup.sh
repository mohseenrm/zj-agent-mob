#!/bin/sh
# Compile the plugin into the recorded session's wasm cache, before recording.
#
#   sh scripts/demo/warmup.sh <session>
#
# Zellij caches compiled wasm under a per-session UUID, so a freshly created
# session is always a cold cache no matter how many times the demo has been
# rendered. The first `pipe --plugin` therefore blocks on a full wasm compile.
#
# That cost is real and variable - measured at 111s on this machine - and it
# landed between act 0 and act 1, pushing a 110s tour to 192s and truncating the
# GIF, because the tape's trailing `Sleep` is a fixed number sized against the
# act log. Paying it here, inside the tape's `Hide` block, moves it off the
# recording and makes the act timings reproducible.
#
# This loads the panel and then closes it again: the tour's own act 0 is what
# should be seen opening it.
set -e

SESSION=${1:?usage: warmup.sh <session>}
WASM="$HOME/.config/zellij/plugins/zj-agent-mob.wasm"
# Must match lib.sh's PLUGIN_CONF exactly. Zellij keys a plugin by
# (url, configuration), so warming a different configuration warms a different
# plugin and the tour still pays the compile.
PLUGIN_CONF="discover=false"

# `launch-or-focus-plugin` is what triggers the compile; it returns as soon as
# the pane exists, so the poll below is what actually waits for readiness.
zellij -s "$SESSION" action launch-or-focus-plugin --floating \
  --configuration "$PLUGIN_CONF" "file:$WASM" </dev/null >/dev/null 2>&1 || true

# Poll until the panel renders something. A plugin pane is only dumpable while
# focused, so focus it first - same reason wait_for() does.
i=0
while [ "$i" -lt 240 ]; do
  rm -f /tmp/zj-warmup.txt
  zellij -s "$SESSION" action dump-screen --path /tmp/zj-warmup.txt </dev/null >/dev/null 2>&1
  if [ -s /tmp/zj-warmup.txt ] && grep -q 'zj-agent-mob' /tmp/zj-warmup.txt 2>/dev/null; then
    break
  fi
  sleep 0.5
  i=$((i + 1))
done

# Leave the session as the tour expects to find it: no panel on screen, so act 0
# is the panel opening rather than the panel already being up.
zellij -s "$SESSION" action close-pane </dev/null >/dev/null 2>&1 || true
rm -f /tmp/zj-warmup.txt
