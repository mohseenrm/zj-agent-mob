#!/bin/sh
# Build a throwaway Zellij config dir for the tour recording.
#
#   sh scripts/demo/tour-config.sh <outdir>
#
# The real config is copied so the personal theme survives; four keys are
# overridden because they cannot be set per-invocation:
#
#   default_layout       - the bar-free layout, so no tab/status bar in frame
#   show_startup_tips    - kills the "About Zellij" modal at the source rather
#                          than racing it with an Escape
#   show_release_notes   - same, on a version bump
#   pane_frames          - hides `Pane #1` titles and, once a client is attached
#                          in two sessions at once, the `MY FOCUS AND:` overlay
#                          Zellij writes into the panel's own top border
set -e

OUT=${1:?usage: tour-config.sh <outdir>}
SRC="$HOME/.config/zellij"

rm -rf "$OUT"
mkdir -p "$OUT/layouts"

if [ -f "$SRC/config.kdl" ]; then
  # Drop existing occurrences of the keys below, including commented ones: a
  # commented `// pane_frames` earlier in the file reads as a duplicate key to
  # Zellij's parser and the appended value is the one that loses.
  grep -vE '^\s*(//\s*)?(default_layout|show_startup_tips|show_release_notes|pane_frames)\s' \
    "$SRC/config.kdl" > "$OUT/config.kdl"
else
  : > "$OUT/config.kdl"
fi

cat >> "$OUT/config.kdl" <<'EOF'

// ---- tour recording overrides (scripts/demo/tour-config.sh) ----
default_layout "tour"
show_startup_tips false
show_release_notes false
pane_frames false
EOF

cat > "$OUT/layouts/tour.kdl" <<'EOF'
// Recording layout: no tab bar, no status bar.
//
// The stock `default` layout stacks a tab-bar above and a status-bar below the
// viewport, costing two rows to chrome the tour never refers to. Used by every
// session the tour creates, including the ones it hops into, so the frame looks
// the same before and after the jump.
layout {
    pane
}
EOF

# Plugin permissions are deliberately NOT copied: Zellij keeps them in its cache
# dir keyed by plugin path rather than by config dir, so the throwaway config
# inherits the existing grant and the panel does not re-prompt on camera.

echo "$OUT"
