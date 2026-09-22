#!/bin/sh
# Build demo/tour.gif.
#
#   ./scripts/demo/make-tour.sh
#
# Two steps, both local and both inspectable:
#
#   cargo test tour::emit   stages each scene as a fleet in memory and renders
#                           it through the panel's own row builders, one text
#                           file per frame (src/tour.rs)
#   render-frames.py        rasterises those frames into the GIF
#
# There is no terminal recorder in the pipeline - no VHS, no ttyd, no headless
# browser - and no Zellij session either. The frames come from `list_item`,
# `detail_item`, `ask_rows`, `subagent_rows` and the real `ribbon` hint sets, so
# what lands in the GIF is what the panel would draw for that fleet.
#
# Staging rather than capturing is what lets the tour show states a live
# recording cannot reach on demand: a rate-limited agent two sessions away, a
# subagent fan-out caught mid-flight, an update waiting to be installed.
#
# The intermediate frames are kept: when something looks wrong in the GIF, the
# frame it came from can be read as text.
set -e

cd "$(dirname "$0")/../.."

FRAMES=${ZJ_TOUR_FRAMES:-/tmp/zj-tour-frames}
export ZJ_TOUR_FRAMES="$FRAMES"

command -v python3 >/dev/null 2>&1 || { echo "python3 not found" >&2; exit 1; }
python3 -c 'import PIL' 2>/dev/null || {
  echo "Pillow not installed: python3 -m pip install pillow" >&2
  exit 1
}

mkdir -p demo
cargo test --quiet tour::emit
python3 scripts/demo/render-frames.py "$FRAMES" demo/tour.gif

echo "frames kept in $FRAMES"
