#!/bin/sh
# Render the demo GIF. Requires vhs (brew install vhs) and a Nerd Font.
#
#   ./scripts/demo/render.sh
#
# Local-only by design: the tape uses the stowed ~/.config/zellij, the personal
# theme, and an installed Nerd Font, none of which exist on a CI runner.
set -e

cd "$(dirname "$0")/../.."

command -v vhs >/dev/null 2>&1 || {
  echo "vhs not found: brew install vhs" >&2
  exit 1
}
[ -f "$HOME/.config/zellij/plugins/zj-agent-mob.wasm" ] || {
  echo "plugin not installed: cargo build --release --target wasm32-wasip1 && ./init.sh" >&2
  exit 1
}

# A leftover session from an interrupted run would be attached rather than
# created, and the tape would record someone else's panes. The tour also builds
# two extra sessions for the cross-session acts; stale ones would show as
# `unknown` rows the moment the new run recreated them.
for s in zjdemo checkout-api platform-infra; do
  zellij kill-session "$s" 2>/dev/null || true
  zellij delete-session "$s" --force 2>/dev/null || true
done

# VHS drives a headless Chrome (via `rod`) against a ttyd server. An interrupted
# render leaves both alive, and they do NOT get reused - the next run starts its
# own pair while the orphans keep holding the port and the CPU. The symptom is a
# render that is intermittently slow, truncated, or simply hangs before the
# staging script ever starts, which cost several runs and reads exactly like a
# bug in the tour.
#
# Matched on the `rod` profile directory rather than on "Chrome", so this can
# only ever kill VHS's own browser and never a real one.
pkill -f 'rod/user-data' 2>/dev/null || true
pkill -f 'ttyd' 2>/dev/null || true
sleep 1

mkdir -p demo
: > /tmp/zj-demo-stage.log
: > /tmp/zj-acts.log

# Built here as well as in the tape: the tape's own call is what the recorded
# shell uses, but building it first means a failure surfaces now rather than as
# a silently chrome-laden recording two minutes from now.
sh scripts/demo/mkconfig.sh /tmp/zj-demo-cfg >/dev/null
[ -f /tmp/zj-demo-cfg/layouts/demo.kdl ] || {
  echo "demo config not built" >&2
  exit 1
}

vhs scripts/demo/tour.tape

# The staging script logs act boundaries; a short log means it died early and the
# recording is of a frozen panel.
# The staging script's own errors come first: a `wait_for` timeout means it
# exited mid-tour, and its message names what never rendered. Without this the
# only symptom is a short GIF and a low act count, which says where it stopped
# but not why.
if [ -s /tmp/zj-demo-stage.log ]; then
  echo "staging reported errors:" >&2
  sed 's/^/  /' /tmp/zj-demo-stage.log >&2
fi

# 12: acts 0-9 plus 3b, plus `done`. Act 10 (reopening the panel from the session
# jumped to) was cut - Zellij cannot give a plugin pane to a session whose only
# clients are nested inside another one.
acts=$(grep -c '^act:' /tmp/zj-acts.log 2>/dev/null || echo 0)
if [ "$acts" -lt 12 ]; then
  echo "WARNING: only $acts/12 acts recorded - see /tmp/zj-demo-stage.log" >&2
  exit 1
fi

# Acts completing is not enough: the staging script runs to the end regardless of
# when the tape stopped recording, so a too-short `Sleep` silently ships a GIF
# that cuts off mid-tour. Compare the tour's own clock against the recording.
#
# `done` is stamped from script start; recording begins at the tape's pre-roll,
# so the GIF has to be at least (done - preroll) seconds long.
PREROLL=20
done_at=$(sed -n 's/^act: +\([0-9]*\)s done$/\1/p' /tmp/zj-acts.log | tail -1)
gif_len=$(ffprobe -v error -show_entries format=duration \
  -of default=noprint_wrappers=1:nokey=1 demo/tour.gif 2>/dev/null | cut -d. -f1)
if [ -n "$done_at" ] && [ -n "$gif_len" ]; then
  need=$((done_at - PREROLL))
  if [ "$gif_len" -lt "$need" ]; then
    echo "WARNING: tour needs ${need}s of recording but the GIF is ${gif_len}s." >&2
    echo "         Raise the trailing Sleep in scripts/demo/tour.tape." >&2
    exit 1
  fi
  # And the other way: a long tail of frozen frames is dead weight in a README.
  if [ "$gif_len" -gt $((need + 12)) ]; then
    echo "note: ${gif_len}s GIF for ${need}s of content - trailing Sleep could drop" \
      "by ~$((gif_len - need))s" >&2
  fi
fi

# The tour leaves both extra sessions running; they are props, not state.
for s in checkout-api platform-infra; do
  zellij delete-session "$s" --force >/dev/null 2>&1 || true
done

echo "demo/tour.gif written ($(du -h demo/tour.gif | cut -f1))"
