#!/bin/sh
# Build a throwaway Zellij config dir for the recording.
#
#   sh scripts/demo/mkconfig.sh <outdir>
#
# Why not just use ~/.config/zellij directly (which docs/demo.md settles on):
# three things the demo needs cannot be set per-invocation.
#
#   - `default_layout` picks the bar-free layout. Passing `-l` instead does NOT
#     work: with `--session` also present, Zellij documents `-l` as adding the
#     layout to that session as a new tab rather than creating a session with it,
#     and in practice the session is never created under the intended name.
#   - `show_startup_tips false` removes the "About Zellij" modal at the source,
#     rather than racing it with an Escape that has to land after the client
#     attaches but before the first frame.
#   - Same for the release-notes popup on a version bump.
#
# The real config is copied first, so the personal theme and keybinds - which
# docs/demo.md deliberately records against - are preserved. Only the three keys
# above are overridden, and only for the recording.
set -e

OUT=${1:?usage: mkconfig.sh <outdir>}
SRC="$HOME/.config/zellij"
DIR=$(cd "$(dirname "$0")" && pwd)

rm -rf "$OUT"
mkdir -p "$OUT/layouts"

# `themes {}` lives in the real config, so copy it rather than re-deriving it.
if [ -f "$SRC/config.kdl" ]; then
  # Drop any existing occurrence of the keys we are about to set, so the
  # appended block cannot collide with one already present.
  grep -vE '^\s*(default_layout|show_startup_tips|show_release_notes|pane_frames)\s' \
    "$SRC/config.kdl" > "$OUT/config.kdl"
else
  : > "$OUT/config.kdl"
fi

cat >> "$OUT/config.kdl" <<'EOF'

// ---- demo recording overrides (scripts/demo/mkconfig.sh) ----
default_layout "demo"
show_startup_tips false
show_release_notes false

// Pane frames carry `Pane #1` / `SCROLL: 0/45` titles the demo never refers to,
// and - once the act-9 hop leaves a client attached in two sessions at once -
// Zellij writes its multi-user `MY FOCUS AND:` indicator into the floating
// panel's own top border, directly over the panel title.
pane_frames false
EOF

# `default_layout` names a layout in the config dir's own layouts/.
cp "$DIR/demo.kdl" "$OUT/layouts/demo.kdl"

# Plugin permissions are NOT copied, deliberately: Zellij keeps them in its
# cache dir (~/Library/Caches/org.Zellij-Contributors.Zellij/permissions.kdl on
# macOS), keyed by plugin path rather than by config dir, so a throwaway config
# inherits the existing grant and the panel does not re-prompt on camera.

echo "$OUT"
