#!/bin/sh
# A stand-in Claude Code / Codex session for the demo's prop panes.
#
#   sh mock-agent.sh <tool> <task> [detail...]
#   ZJ_MOCK_PROMPT="rm -rf node_modules" sh mock-agent.sh <tool> <task> [detail...]
#
# With ZJ_MOCK_PROMPT set, the transcript ends on a permission prompt waiting on
# an answer, which is what the panel's `waiting` row means. That is the pane the
# cross-session jump lands on, so it has to show the thing being waited for.
#
# The panel is the subject of the demo, but the panes behind it are visible the
# moment you jump to one - and an empty `sleep` pane gives the game away. This
# prints a transcript shaped like a real session, then idles forever so the pane
# outlives the recording (reconcile() culls any agent whose pane is gone).
#
# Nothing here talks to the plugin: statuses are piped separately by
# stage-tour.sh. This is set dressing, and it is deliberately obvious about
# that in the source so nobody mistakes it for a working agent.

TOOL=${1:-claude}
TASK=${2:-Refactor the auth middleware}
shift 2 2>/dev/null || true

# Dim / accent / reset. The panel uses the Zellij theme; a pane cannot, so these
# are plain ANSI and deliberately muted so the pane never outshines the panel.
D=$(printf '\033[2m')
A=$(printf '\033[38;5;110m')
G=$(printf '\033[38;5;108m')
R=$(printf '\033[0m')

say() { printf '%s\n' "$1"; sleep "${2:-0.35}"; }

clear 2>/dev/null || true

if [ "$TOOL" = codex ]; then
  say "${A}codex${R} ${D}v0.47.0${R}"
else
  say "${A}✻ Claude Code${R} ${D}v2.1.4${R}"
fi
say "${D}$(pwd 2>/dev/null || echo ~/repo)${R}" 0.5
say ""
say "${D}>${R} $TASK" 0.6
say ""

# One line per remaining argument, rendered as a tool call the way the real
# transcript does. The panel's detail line shows the same call.
for step in "$@"; do
  say "${G}⏺${R} $step" 0.7
done

if [ -n "${ZJ_MOCK_PROMPT:-}" ]; then
  # The blocked state: the agent has stopped and is waiting on a human. This is
  # what a `waiting` row in the panel corresponds to, and what the panel's own
  # approve/reject box answers without you coming here at all.
  Y=$(printf '\033[38;5;179m')
  say ""
  say "${Y}⏺${R} Bash"
  say "  ${D}│${R} ${ZJ_MOCK_PROMPT}"
  say ""
  say "  ${Y}Do you want to proceed?${R}"
  say "  ${A}❯ 1. Yes${R}"
  say "  ${D}  2. Yes, and don't ask again${R}"
  say "  ${D}  3. No, and tell Claude what to do differently${R}"
  say ""
else
  say ""
  say "${D}esc to interrupt${R}" 0.2
fi

# The pane has to outlive the tour: a dead pane means a culled agent row.
while true; do
  sleep 3600
done
