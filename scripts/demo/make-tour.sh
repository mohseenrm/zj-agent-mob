#!/bin/sh
# Build demo/tour.gif.
#
#   ./scripts/demo/make-tour.sh
#
# Two steps, both local and both inspectable:
#
#   capture-tour.sh   drives a throwaway Zellij session and reads the panel's
#                     own output back as text, one file per frame
#   render-frames.py  rasterises those frames into the GIF
#
# There is no terminal recorder in the pipeline - no VHS, no ttyd, no headless
# browser - so there is no recorded shell whose environment, prompt or window
# chrome has to be fought. What lands in the GIF is what the plugin rendered.
#
# The intermediate frames are kept: when something looks wrong in the GIF, the
# frame it came from can be read as text.
set -e

cd "$(dirname "$0")/../.."

FRAMES=${ZJ_TOUR_FRAMES:-/tmp/zj-tour-frames}

[ -f "$HOME/.config/zellij/plugins/zj-agent-mob.wasm" ] || {
  echo "plugin not installed: cargo build --release --target wasm32-wasip1 && ./init.sh" >&2
  exit 1
}
command -v python3 >/dev/null 2>&1 || { echo "python3 not found" >&2; exit 1; }
python3 -c 'import PIL' 2>/dev/null || {
  echo "Pillow not installed: python3 -m pip install pillow" >&2
  exit 1
}

mkdir -p demo
sh scripts/demo/capture-tour.sh "$FRAMES"
python3 scripts/demo/render-frames.py "$FRAMES" demo/tour.gif

echo "frames kept in $FRAMES"
