#!/bin/sh
# zj-agent-mob one-time installer.
#
#   ./init.sh              install hooks + plugin
#   ./init.sh uninstall    remove exactly what was installed
#   ./init.sh --dry-run    show what would change, write nothing
#
# Installs the status hook into Claude Code (~/.claude/settings.json) and
# Codex (~/.codex/hooks.json), and copies the plugin to the zellij plugin dir.
#
# Symlink-aware: these files are commonly stow-managed symlinks into a dotfiles
# repo. We resolve to the real path before the temp-file swap, otherwise `mv`
# replaces the symlink with a regular file and detaches it from the repo.

set -eu

SRC_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
HOOK_SRC="$SRC_DIR/scripts/zj-agent-mob-hook.sh"
# Use the bin artifact (hyphenated). Zellij needs the WASI `_start` export,
# which the cdylib (zj_agent_mob.wasm, underscores) does not have.
WASM_DIR="$SRC_DIR/target/wasm32-wasip1/release"
WASM_SRC="$WASM_DIR/zj-agent-mob.wasm"

HOOK_DIR="${ZJ_AGENT_HOOK_DIR:-$HOME/.config/zj-agent-mob}"
HOOK_DST="$HOOK_DIR/hook.sh"
PLUGIN_DIR="${ZJ_AGENT_PLUGIN_DIR:-$HOME/.config/zellij/plugins}"
PLUGIN_DST="$PLUGIN_DIR/zj-agent-mob.wasm"

CLAUDE_SETTINGS="${CLAUDE_CONFIG_DIR:-$HOME/.claude}/settings.json"
CODEX_HOOKS="${CODEX_HOME:-$HOME/.codex}/hooks.json"

MODE=install
DRY=0
for arg in "$@"; do
  case "$arg" in
    uninstall) MODE=uninstall ;;
    --dry-run) DRY=1 ;;
    -h|--help) sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

say()  { printf '%s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die()  { printf 'error: %s\n' "$*" >&2; exit 1; }

command -v jq >/dev/null 2>&1 || die "jq is required (brew install jq)"

# Resolve symlinks so we write through to the real file (e.g. a dotfiles repo).
# `readlink -f` is GNU/BSD-modern; fall back to a portable loop for older systems.
resolve() {
  if readlink -f "$1" >/dev/null 2>&1; then
    readlink -f "$1"
    return
  fi
  _p=$1
  while [ -L "$_p" ]; do
    _l=$(readlink "$_p")
    case "$_l" in
      /*) _p=$_l ;;
      *)  _p=$(dirname "$_p")/$_l ;;
    esac
  done
  # Normalize
  (CDPATH= cd -- "$(dirname -- "$_p")" && printf '%s/%s\n' "$(pwd)" "$(basename -- "$_p")")
}

# Atomically write stdin to $1, preserving symlinks by resolving first.
write_atomic() {
  _target=$1
  if [ -e "$_target" ] || [ -L "$_target" ]; then
    _real=$(resolve "$_target")
  else
    _real=$_target
  fi
  if [ "$DRY" = 1 ]; then
    say "  [dry-run] would write $_real"
    cat >/dev/null
    return
  fi
  mkdir -p "$(dirname "$_real")"
  cat > "$_real.zjtmp"
  mv "$_real.zjtmp" "$_real"
  if [ "$_real" != "$_target" ]; then
    say "  wrote $_real (via symlink $_target)"
  else
    say "  wrote $_real"
  fi
}

backup() {
  _t=$1
  [ -e "$_t" ] || return 0
  _real=$(resolve "$_t")
  _bak="$_real.bak-$(date +%Y%m%d%H%M%S)"
  if [ "$DRY" = 1 ]; then
    say "  [dry-run] would back up -> $_bak"
    return 0
  fi
  cp "$_real" "$_bak"
  say "  backed up -> $_bak"
}

# ---------------------------------------------------------------- claude code

# Build the hooks block. Matchers keep Notification scoped to the events that
# actually mean "needs you". `async: true` so a hook can never block a turn.
claude_hooks_json() {
  jq -n --arg cmd "$HOOK_DST" '
    def h($extra): {type:"command", command:$cmd, async:true} + $extra;
    {
      SessionStart:     [{matcher:"*",                        hooks:[h({})]}],
      UserPromptSubmit: [{                                    hooks:[h({})]}],
      PreToolUse:       [{matcher:"*",                        hooks:[h({})]}],
      PostToolUse:      [{matcher:"*",                        hooks:[h({})]}],
      Notification:     [{matcher:"permission_prompt|idle_prompt", hooks:[h({})]}],
      Stop:             [{                                    hooks:[h({})]}],
      SessionEnd:       [{matcher:"*",                        hooks:[h({})]}]
    }'
}

install_claude() {
  say "Claude Code: $CLAUDE_SETTINGS"
  [ -e "$CLAUDE_SETTINGS" ] || { mkdir -p "$(dirname "$CLAUDE_SETTINGS")"; echo '{}' > "$CLAUDE_SETTINGS"; }
  backup "$CLAUDE_SETTINGS"
  _hooks=$(claude_hooks_json)
  # Merge: for each event, drop any entry already pointing at our hook (so
  # re-running is idempotent), then append ours.
  jq --argjson new "$_hooks" --arg cmd "$HOOK_DST" '
    .hooks = ((.hooks // {}) as $old
      | reduce ($new | keys_unsorted[]) as $ev ($old;
          .[$ev] = (
            (($old[$ev] // [])
              | map(.hooks |= map(select(.command != $cmd))
                    | select((.hooks | length) > 0)))
            + $new[$ev]
          )))
  ' "$(resolve "$CLAUDE_SETTINGS")" | write_atomic "$CLAUDE_SETTINGS"
}

uninstall_claude() {
  [ -e "$CLAUDE_SETTINGS" ] || return 0
  say "Claude Code: $CLAUDE_SETTINGS"
  jq --arg cmd "$HOOK_DST" '
    if .hooks then
      .hooks |= with_entries(
        .value |= (map(.hooks |= map(select(.command != $cmd))
                       | select((.hooks | length) > 0))))
      | .hooks |= with_entries(select((.value | length) > 0))
      | if (.hooks | length) == 0 then del(.hooks) else . end
    else . end
  ' "$(resolve "$CLAUDE_SETTINGS")" | write_atomic "$CLAUDE_SETTINGS"
}

# ---------------------------------------------------------------------- codex

# Codex has no async flag and SessionEnd defaults to a 1s timeout, so the hook
# must be fast. ZJ_AGENT_TOOL=codex selects the codex transcript reader.
codex_hooks_json() {
  jq -n --arg cmd "env ZJ_AGENT_TOOL=codex $HOOK_DST" '
    def h: {type:"command", command:$cmd};
    { hooks: {
        SessionStart:     [{matcher:"*", hooks:[h]}],
        UserPromptSubmit: [{             hooks:[h]}],
        PreToolUse:       [{matcher:"*", hooks:[h]}],
        PostToolUse:      [{matcher:"*", hooks:[h]}],
        PermissionRequest:[{matcher:"*", hooks:[h]}],
        Stop:             [{             hooks:[h]}],
        SessionEnd:       [{matcher:"*", hooks:[h]}]
      }}'
}

install_codex() {
  say "Codex: $CODEX_HOOKS"
  _cmd="env ZJ_AGENT_TOOL=codex $HOOK_DST"
  if [ -e "$CODEX_HOOKS" ]; then
    backup "$CODEX_HOOKS"
    jq --argjson new "$(codex_hooks_json)" --arg cmd "$_cmd" '
      .hooks = ((.hooks // {}) as $old
        | reduce ($new.hooks | keys_unsorted[]) as $ev ($old;
            .[$ev] = (
              (($old[$ev] // [])
                | map(.hooks |= map(select(.command != $cmd))
                      | select((.hooks | length) > 0)))
              + $new.hooks[$ev]
            )))
    ' "$(resolve "$CODEX_HOOKS")" | write_atomic "$CODEX_HOOKS"
  else
    codex_hooks_json | write_atomic "$CODEX_HOOKS"
  fi
}

uninstall_codex() {
  [ -e "$CODEX_HOOKS" ] || return 0
  say "Codex: $CODEX_HOOKS"
  _cmd="env ZJ_AGENT_TOOL=codex $HOOK_DST"
  jq --arg cmd "$_cmd" '
    if .hooks then
      .hooks |= with_entries(
        .value |= (map(.hooks |= map(select(.command != $cmd))
                       | select((.hooks | length) > 0))))
      | .hooks |= with_entries(select((.value | length) > 0))
    else . end
  ' "$(resolve "$CODEX_HOOKS")" | write_atomic "$CODEX_HOOKS"
}

# ----------------------------------------------------------------------- main

if [ "$MODE" = uninstall ]; then
  say "Uninstalling zj-agent-mob..."
  uninstall_claude
  uninstall_codex
  if [ "$DRY" = 0 ]; then
    rm -f "$HOOK_DST" && say "removed $HOOK_DST"
    rm -f "$PLUGIN_DST" && say "removed $PLUGIN_DST"
  else
    say "  [dry-run] would remove $HOOK_DST and $PLUGIN_DST"
  fi
  say ""
  say "Done. Backups (*.bak-*) were left in place."
  exit 0
fi

[ -f "$HOOK_SRC" ] || die "hook script not found: $HOOK_SRC"

say "Installing zj-agent-mob..."
[ "$DRY" = 1 ] && say "(dry run: nothing will be written)"
say ""

if [ "$DRY" = 0 ]; then
  mkdir -p "$HOOK_DIR" "$PLUGIN_DIR"
  cp "$HOOK_SRC" "$HOOK_DST"
  chmod +x "$HOOK_DST"
  say "hook  -> $HOOK_DST"
else
  say "  [dry-run] would install hook -> $HOOK_DST"
fi

if [ -f "$WASM_SRC" ]; then
  # Guard against installing a cdylib-only build: without `_start` Zellij fails
  # at load time with "could not find exported function".
  if ! grep -q '_start' "$WASM_SRC" 2>/dev/null; then
    warn "$WASM_SRC has no _start export (cdylib instead of bin?)."
    warn "Rebuild with: cargo build --release --target wasm32-wasip1"
  fi
  if [ "$DRY" = 0 ]; then
    cp "$WASM_SRC" "$PLUGIN_DST"
    say "plugin -> $PLUGIN_DST"
  else
    say "  [dry-run] would install plugin -> $PLUGIN_DST"
  fi
else
  warn "plugin not built; run: cargo build --release --target wasm32-wasip1"
fi
say ""

install_claude
say ""
install_codex
say ""

say "Done. Next steps:"
say ""
say "  1. Add a keybinding to ~/.config/zellij/config.kdl:"
say ""
say '     shared_except "locked" {'
say '         bind "Ctrl a" {'
say "             LaunchOrFocusPlugin \"file:$PLUGIN_DST\" {"
say '                 floating true; move_to_focused_tab true'
say '             }'
say '         }'
say '     }'
say ""
say "  2. Restart any running claude/codex sessions so they pick up the hooks."
say ""

# A stow-managed config lives in a git repo; the install shows up as a diff.
for f in "$CLAUDE_SETTINGS" "$CODEX_HOOKS"; do
  [ -L "$f" ] || continue
  _real=$(resolve "$f")
  say "note: $f -> $_real"
  say "      That file is a symlink (likely stow/dotfiles). The hooks were written"
  say "      through to the real path, so commit them in that repo if you want them"
  say "      version-controlled."
done
