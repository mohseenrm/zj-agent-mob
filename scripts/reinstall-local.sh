#!/bin/sh
# Build from this checkout and install it over whatever is already there.
#
# init.sh already overwrites unconditionally; what it cannot do is make a
# RUNNING zellij drop the old wasm. Zellij caches compiled plugins per session,
# so a fresh binary on disk is not the binary in memory. This clears that cache
# and the stale spool records that outlive a hook change.
#
#   ./scripts/reinstall-local.sh          build, install, clear caches
#   ./scripts/reinstall-local.sh --check  verify installed == this checkout
set -eu

cd "$(dirname "$0")/.."

WASM=target/wasm32-wasip1/release/zj-agent-mob.wasm
DST="${ZJ_AGENT_PLUGIN_DIR:-$HOME/.config/zellij/plugins}/zj-agent-mob.wasm"
HOOK_DST="${ZJ_AGENT_HOOK_DIR:-$HOME/.config/zj-agent-mob}/hook.sh"
SPOOL="${ZJ_AGENT_SPOOL_DIR:-${TMPDIR:-/tmp}/zj-agent-mob-$(id -u)/status}"

sum() { shasum -a 256 "$1" 2>/dev/null | cut -d' ' -f1; }

if [ "${1:-}" = "--check" ]; then
  status=0
  if [ "$(sum "$WASM")" = "$(sum "$DST")" ] && [ -n "$(sum "$DST")" ]; then
    echo "wasm  ok   $DST"
  else
    echo "wasm  STALE $DST"
    status=1
  fi
  if cmp -s scripts/zj-agent-mob-hook.sh "$HOOK_DST"; then
    echo "hook  ok   $HOOK_DST"
  else
    echo "hook  STALE $HOOK_DST"
    status=1
  fi
  exit $status
fi

cargo build --release --target wasm32-wasip1
./init.sh

# Records whose status is empty cannot be parsed, so the plugin skips them on
# every poll and the row sits at `unknown` forever. Older hooks wrote these.
if [ -d "$SPOOL" ]; then
  for f in "$SPOOL"/*.[0-9]*; do
    [ -f "$f" ] || continue
    case "$(head -n 1 "$f")" in
      *,status=,*) echo "clearing statusless record: ${f##*/}"; rm -f "$f" ;;
    esac
  done
fi

# Zellij caches compiled plugins; without this a running session keeps serving
# the old wasm no matter what is on disk.
for c in "$HOME/Library/Caches/org.Zellij-Contributors.Zellij" "$HOME/.cache/zellij"; do
  if [ -d "$c" ]; then
    rm -rf "$c"
    echo "cleared plugin cache: $c"
  fi
done

echo
echo "Installed from this checkout. To pick it up:"
echo "  1. restart claude/codex sessions  (hooks are read at session start)"
echo "  2. start a NEW zellij session     (the reliable plugin reload)"
