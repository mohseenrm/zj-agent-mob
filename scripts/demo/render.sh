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
# created, and the tape would record someone else's panes.
zellij kill-session zjdemo 2>/dev/null || true
zellij delete-session zjdemo 2>/dev/null || true

mkdir -p demo
: > /tmp/zj-demo-stage.log

vhs scripts/demo/tour.tape

# The staging script logs act boundaries; a short log means it died early and the
# recording is of a frozen panel.
acts=$(grep -c '^act:' /tmp/zj-acts.log 2>/dev/null || echo 0)
if [ "$acts" -lt 8 ]; then
  echo "WARNING: only $acts/8 acts recorded - see /tmp/zj-demo-stage.log" >&2
  exit 1
fi

echo "demo/tour.gif written ($(du -h demo/tour.gif | cut -f1))"
