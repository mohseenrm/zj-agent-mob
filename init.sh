#!/bin/sh
# zj-agent-mob installer.
#
#   ./init.sh                    install hooks + plugin
#   ./init.sh uninstall          remove exactly what was installed
#   ./init.sh --dry-run          show what would change, write nothing
#   ./init.sh status             print install state (machine-readable)
#   ./init.sh install claude     install one target only
#   ./init.sh install claude codex   install several targets
#   ./init.sh uninstall codex    remove one target only
#
# Targets: claude, codex, plugin. Omitting the target means all of them.
#
# Writes the status hook into ~/.claude/settings.json and ~/.codex/hooks.json,
# and copies the plugin to the zellij plugin dir. Also self-copies to
# ~/.config/zj-agent-mob/install.sh, which is what the plugin's install screen
# drives so it need not know where the repo was cloned.
#
# Symlink-aware: settings files are often stow-managed symlinks, so we resolve
# to the real path before the temp-file swap. Otherwise `mv` would replace the
# symlink with a regular file and detach it from the dotfiles repo.

set -eu

# shellcheck disable=SC1007  # `CDPATH= cd` clears CDPATH for this command only.
SRC_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
HOOK_SRC="$SRC_DIR/scripts/zj-agent-mob-hook.sh"
# Use the bin artifact (hyphenated). Zellij needs the WASI `_start` export,
# which the cdylib (zj_agent_mob.wasm, underscores) does not have.
WASM_DIR="$SRC_DIR/target/wasm32-wasip1/release"
WASM_SRC="$WASM_DIR/zj-agent-mob.wasm"

HOOK_DIR="${ZJ_AGENT_HOOK_DIR:-$HOME/.config/zj-agent-mob}"
HOOK_DST="$HOOK_DIR/hook.sh"
SELF_DST="$HOOK_DIR/install.sh"

# The command string written into the agents' settings files. Kept separate from
# HOOK_DST (the real path, used for filesystem work) so settings can carry a
# literal `$HOME/...`: both agents run hook commands through a shell, which
# expands it at call time. That keeps a username out of files people commit to a
# dotfiles repo. Paths outside $HOME have nothing to substitute and stay absolute.
case "$HOOK_DST" in
  "$HOME"/*) HOOK_CMD="\$HOME${HOOK_DST#"$HOME"}" ;;
  *)         HOOK_CMD="$HOOK_DST" ;;
esac
CODEX_CMD="env ZJ_AGENT_TOOL=codex $HOOK_CMD"

# Installs written before the $HOME rewrite hardcoded the absolute path. Match it
# too, so uninstall and the idempotent re-install still recognize their entries.
HOOK_CMD_LEGACY="$HOOK_DST"
CODEX_CMD_LEGACY="env ZJ_AGENT_TOOL=codex $HOOK_DST"
PLUGIN_DIR="${ZJ_AGENT_PLUGIN_DIR:-$HOME/.config/zellij/plugins}"
PLUGIN_DST="$PLUGIN_DIR/zj-agent-mob.wasm"

CLAUDE_SETTINGS="${CLAUDE_CONFIG_DIR:-$HOME/.claude}/settings.json"
CODEX_HOOKS="${CODEX_HOME:-$HOME/.codex}/hooks.json"

MODE=install
DRY=0
TARGET=all
for arg in "$@"; do
  case "$arg" in
    install)   MODE=install ;;
    uninstall) MODE=uninstall ;;
    status)    MODE=status ;;
    # Targets accumulate, so `install claude codex` does both in one run. The
    # first explicit target replaces the "all" default rather than adding to it.
    claude|codex|plugin)
      case "$TARGET" in
        all) TARGET=$arg ;;
        *)   TARGET="$TARGET $arg" ;;
      esac ;;
    --dry-run) DRY=1 ;;
    # Print the header block: every comment line until the first blank line, so
    # the help text cannot drift out of sync with a hardcoded line range.
    -h|--help) sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

# True when $TARGET selects $1 (an explicit target in the list, or "all").
wants() {
  [ "$TARGET" = all ] && return 0
  for _t in $TARGET; do
    [ "$_t" = "$1" ] && return 0
  done
  return 1
}

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
  # shellcheck disable=SC1007  # `CDPATH= cd` clears CDPATH for this command only.
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
  jq -n --arg cmd "$HOOK_CMD" '
    def h($extra): {type:"command", command:$cmd, async:true} + $extra;
    {
      SessionStart:       [{matcher:"*",                      hooks:[h({})]}],
      UserPromptSubmit:   [{                                  hooks:[h({})]}],
      PreToolUse:         [{matcher:"*",                      hooks:[h({})]}],
      PostToolUse:        [{matcher:"*",                      hooks:[h({})]}],
      PostToolUseFailure: [{matcher:"*",                      hooks:[h({})]}],
      Notification:       [{matcher:"permission_prompt|idle_prompt", hooks:[h({})]}],
      PermissionRequest:  [{matcher:"*",                      hooks:[h({async:false})]}],
      SubagentStart:      [{matcher:"*",                      hooks:[h({})]}],
      SubagentStop:       [{matcher:"*",                      hooks:[h({})]}],
      TaskCreated:        [{                                  hooks:[h({})]}],
      TaskCompleted:      [{                                  hooks:[h({})]}],
      PreCompact:         [{matcher:"*",                      hooks:[h({})]}],
      PostCompact:        [{matcher:"*",                      hooks:[h({})]}],
      Stop:               [{                                  hooks:[h({})]}],
      StopFailure:        [{matcher:"*",                      hooks:[h({})]}],
      SessionEnd:         [{matcher:"*",                      hooks:[h({})]}]
    }'
}

install_claude() {
  say "Claude Code: $CLAUDE_SETTINGS"
  # A dry run must not create the settings file it is only pretending to edit,
  # so seed on disk for a real install and merge into an in-memory `{}` otherwise.
  if [ -e "$CLAUDE_SETTINGS" ]; then
    _current=$(cat "$(resolve "$CLAUDE_SETTINGS")")
  else
    [ "$DRY" = 1 ] || { mkdir -p "$(dirname "$CLAUDE_SETTINGS")"; echo '{}' > "$CLAUDE_SETTINGS"; }
    _current='{}'
  fi
  backup "$CLAUDE_SETTINGS"
  _hooks=$(claude_hooks_json)
  # Merge: for each event, drop any entry already pointing at our hook (so
  # re-running is idempotent), then append ours.
  jq --argjson new "$_hooks" --arg cmd "$HOOK_CMD" --arg old_cmd "$HOOK_CMD_LEGACY" '
    .hooks = ((.hooks // {}) as $old
      | reduce ($new | keys_unsorted[]) as $ev ($old;
          .[$ev] = (
            (($old[$ev] // [])
              | map(.hooks |= map(select(.command != $cmd and .command != $old_cmd))
                    | select((.hooks | length) > 0)))
            + $new[$ev]
          )))
  ' <<EOF | write_atomic "$CLAUDE_SETTINGS"
$_current
EOF
}

uninstall_claude() {
  [ -e "$CLAUDE_SETTINGS" ] || return 0
  say "Claude Code: $CLAUDE_SETTINGS"
  jq --arg cmd "$HOOK_CMD" --arg old_cmd "$HOOK_CMD_LEGACY" '
    if .hooks then
      .hooks |= with_entries(
        .value |= (map(.hooks |= map(select(.command != $cmd and .command != $old_cmd))
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
  jq -n --arg cmd "$CODEX_CMD" '
    def h: {type:"command", command:$cmd};
    { hooks: {
        SessionStart:     [{matcher:"*", hooks:[h]}],
        UserPromptSubmit: [{             hooks:[h]}],
        PreToolUse:       [{matcher:"*", hooks:[h]}],
        PostToolUse:      [{matcher:"*", hooks:[h]}],
        PermissionRequest:[{matcher:"*", hooks:[h]}],
        SubagentStart:    [{matcher:"*", hooks:[h]}],
        SubagentStop:     [{matcher:"*", hooks:[h]}],
        PreCompact:       [{matcher:"*", hooks:[h]}],
        PostCompact:      [{matcher:"*", hooks:[h]}],
        Stop:             [{             hooks:[h]}],
        SessionEnd:       [{matcher:"*", hooks:[h]}]
      }}'
}

install_codex() {
  say "Codex: $CODEX_HOOKS"
  if [ -e "$CODEX_HOOKS" ]; then
    backup "$CODEX_HOOKS"
    jq --argjson new "$(codex_hooks_json)" --arg cmd "$CODEX_CMD" --arg old_cmd "$CODEX_CMD_LEGACY" '
      .hooks = ((.hooks // {}) as $old
        | reduce ($new.hooks | keys_unsorted[]) as $ev ($old;
            .[$ev] = (
              (($old[$ev] // [])
                | map(.hooks |= map(select(.command != $cmd and .command != $old_cmd))
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
  jq --arg cmd "$CODEX_CMD" --arg old_cmd "$CODEX_CMD_LEGACY" '
    if .hooks then
      .hooks |= with_entries(
        .value |= (map(.hooks |= map(select(.command != $cmd and .command != $old_cmd))
                       | select((.hooks | length) > 0))))
      | .hooks |= with_entries(select((.value | length) > 0))
    else . end
  ' "$(resolve "$CODEX_HOOKS")" | write_atomic "$CODEX_HOOKS"
}

# ---------------------------------------------------------------------- status

# Does $1 (a settings file) reference our hook command, in either the current
# `$HOME`-relative form ($2) or the older absolute one ($3)?
hooked() {
  [ -e "$1" ] || return 1
  jq -e --arg cmd "$2" --arg old_cmd "$3" '
    [(.hooks // {}) | .[]? | .[]? | .hooks[]? | .command]
      | (index($cmd) != null) or (index($old_cmd) != null)
  ' "$(resolve "$1")" >/dev/null 2>&1
}

# One `key=state` line per target. Parsed by the plugin's install screen, so the
# format is fixed: state is `installed` or `absent`.
print_status() {
  hooked "$CLAUDE_SETTINGS" "$HOOK_CMD" "$HOOK_CMD_LEGACY" && _c=installed || _c=absent
  hooked "$CODEX_HOOKS" "$CODEX_CMD" "$CODEX_CMD_LEGACY" && _x=installed || _x=absent
  [ -f "$PLUGIN_DST" ] && _p=installed || _p=absent
  [ -x "$HOOK_DST" ] && _h=installed || _h=absent
  say "claude=$_c"
  say "codex=$_x"
  say "plugin=$_p"
  say "hook=$_h"
}

# ----------------------------------------------------------------------- main

if [ "$MODE" = status ]; then
  print_status
  exit 0
fi

if [ "$MODE" = uninstall ]; then
  say "Uninstalling zj-agent-mob..."
  if wants claude; then uninstall_claude; fi
  if wants codex;  then uninstall_codex;  fi
  if [ "$DRY" = 0 ]; then
    if wants plugin; then
      rm -f "$PLUGIN_DST"
      say "removed $PLUGIN_DST"
    fi
    # hook.sh and install.sh are shared by both agents, so they only go away on
    # a full uninstall.
    if [ "$TARGET" = all ]; then
      rm -f "$HOOK_DST" "$SELF_DST"
      say "removed $HOOK_DST"
    fi
  else
    say "  [dry-run] would remove install artifacts"
  fi
  say ""
  say "Done. Backups (*.bak-*) were left in place."
  exit 0
fi

# Reinstalling a single agent's hooks only needs the already-installed hook.sh,
# so the source tree is optional there.
if [ ! -f "$HOOK_SRC" ]; then
  if [ -x "$HOOK_DST" ]; then
    HOOK_SRC=$HOOK_DST
  else
    die "hook script not found: $HOOK_SRC"
  fi
fi

say "Installing zj-agent-mob..."
if [ "$DRY" = 1 ]; then say "(dry run: nothing will be written)"; fi
say ""

if [ "$DRY" = 0 ]; then
  mkdir -p "$HOOK_DIR"
  # Re-running from the installed copy makes src and dst the same file, and
  # `cp foo foo` is an error that would abort the rest of the install.
  same_file() { [ -e "$1" ] && [ -e "$2" ] && [ "$(resolve "$1")" = "$(resolve "$2")" ]; }
  if same_file "$HOOK_SRC" "$HOOK_DST"; then
    say "hook  -> $HOOK_DST (already current)"
  else
    cp "$HOOK_SRC" "$HOOK_DST"
    say "hook  -> $HOOK_DST"
  fi
  chmod +x "$HOOK_DST"
  # Self-copy so the plugin's install screen has a stable path to drive,
  # independent of where this repo was cloned.
  if same_file "$0" "$SELF_DST"; then
    say "installer -> $SELF_DST (already current)"
  else
    cp "$0" "$SELF_DST"
    say "installer -> $SELF_DST"
  fi
  chmod +x "$SELF_DST"
else
  say "  [dry-run] would install hook -> $HOOK_DST"
  say "  [dry-run] would install installer -> $SELF_DST"
fi

if wants plugin; then
  if [ -f "$WASM_SRC" ]; then
    # Guard against installing a cdylib-only build: without `_start` Zellij fails
    # at load time with "could not find exported function".
    # -a: without it grep treats the wasm as binary and reports no match even
    # when the export is present, so the guard would fire on every good build.
    if ! grep -qa '_start' "$WASM_SRC" 2>/dev/null; then
      warn "$WASM_SRC has no _start export (cdylib instead of bin?)."
      warn "Rebuild with: cargo build --release --target wasm32-wasip1"
    fi
    if [ "$DRY" = 0 ]; then
      mkdir -p "$PLUGIN_DIR"
      cp "$WASM_SRC" "$PLUGIN_DST"
      say "plugin -> $PLUGIN_DST"
    else
      say "  [dry-run] would install plugin -> $PLUGIN_DST"
    fi
  elif [ "$TARGET" = plugin ]; then
    die "plugin not built; run: cargo build --release --target wasm32-wasip1"
  else
    warn "plugin not built; run: cargo build --release --target wasm32-wasip1"
  fi
fi
say ""

if wants claude; then
  install_claude
  say ""
fi
if wants codex; then
  install_codex
  say ""
fi

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
