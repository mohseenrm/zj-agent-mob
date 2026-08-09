#!/bin/sh
# End-to-end tests for init.sh.
#
# The installer is the only supported path to a working install, and what it
# writes is read by Claude Code and Codex themselves: if an event name, matcher
# or flag is wrong, every agent reports nothing and no test in the Rust suite
# notices. These drive the real script against a throwaway HOME and assert on
# the resulting settings files.
#
# Nothing here touches the real ~/.claude or ~/.codex: every path the installer
# uses is overridden per-run (ZJ_AGENT_HOOK_DIR, ZJ_AGENT_PLUGIN_DIR,
# CLAUDE_CONFIG_DIR, CODEX_HOME).
#
#   ./tests/e2e-install.sh

set -eu

# shellcheck disable=SC1007  # `CDPATH= cd` clears CDPATH for this command only.
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
INIT="$ROOT/init.sh"

command -v jq >/dev/null 2>&1 || { echo "SKIP: jq not installed"; exit 0; }

PASS=0
FAIL=0

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT INT TERM

ok()  { PASS=$((PASS + 1)); printf '  ok   %s\n' "$1"; }
bad() { FAIL=$((FAIL + 1)); printf '  FAIL %s\n     %s\n' "$1" "$2"; }

# assert_eq <name> <got> <want>
assert_eq() {
  if [ "$2" = "$3" ]; then ok "$1"; else bad "$1" "want '$3', got '$2'"; fi
}

# assert_contains <name> <haystack> <needle>
assert_contains() {
  case "$2" in
    *"$3"*) ok "$1" ;;
    *) bad "$1" "expected to contain '$3', got: ${2:-<empty>}" ;;
  esac
}

# Each case gets a pristine set of install paths, so no case can be masked by
# another's leftovers.
CASE=0
fresh() {
  CASE=$((CASE + 1))
  SANDBOX="$WORK/case$CASE"
  mkdir -p "$SANDBOX"
  ZJ_AGENT_HOOK_DIR="$SANDBOX/config/zj-agent-mob"
  ZJ_AGENT_PLUGIN_DIR="$SANDBOX/config/zellij/plugins"
  CLAUDE_CONFIG_DIR="$SANDBOX/claude"
  CODEX_HOME="$SANDBOX/codex"
  # The loop-closed check below runs the installed hook for real, which writes a
  # status record. Without this it lands in the user's own spool.
  ZJ_AGENT_SPOOL_DIR="$SANDBOX/spool"
  export ZJ_AGENT_HOOK_DIR ZJ_AGENT_PLUGIN_DIR CLAUDE_CONFIG_DIR CODEX_HOME ZJ_AGENT_SPOOL_DIR
  CLAUDE_JSON="$CLAUDE_CONFIG_DIR/settings.json"
  CODEX_JSON="$CODEX_HOME/hooks.json"
  HOOK_CMD="$ZJ_AGENT_HOOK_DIR/hook.sh"
}

# init <args...> -> runs the installer quietly, failing the run if it errors.
init() {
  sh "$INIT" "$@" >"$WORK/out" 2>"$WORK/err" || {
    bad "init.sh $*" "exited non-zero: $(cat "$WORK/err")"
    return 1
  }
}

# The set of hook commands registered for one event, one per line.
cmds_for() { jq -r --arg ev "$2" '[.hooks[$ev][]?.hooks[]?.command] | .[]' "$1"; }

echo "installer basics"
fresh
assert_contains "help exits 0 and prints usage" "$(sh "$INIT" --help)" "uninstall"
rc=0; sh "$INIT" --bogus >/dev/null 2>&1 || rc=$?
assert_eq "unknown argument is rejected" "$rc" "2"

fresh
init install claude || true
if [ -x "$HOOK_CMD" ]; then ok "hook.sh is installed executable"; else
  bad "hook.sh is installed executable" "not executable at $HOOK_CMD"; fi
# The plugin's install screen drives this copy, not the repo, so it must exist
# and run standalone.
if [ -x "$ZJ_AGENT_HOOK_DIR/install.sh" ]; then ok "installer self-copies"; else
  bad "installer self-copies" "missing $ZJ_AGENT_HOOK_DIR/install.sh"; fi

echo
echo "claude code hook contract"
fresh
init install claude || true
# Claude reads these event names verbatim; a typo silences the plugin entirely.
for ev in SessionStart UserPromptSubmit PreToolUse PostToolUse Notification Stop SessionEnd; do
  assert_contains "claude registers $ev" "$(cmds_for "$CLAUDE_JSON" "$ev")" "$HOOK_CMD"
done
# Every hook event the installer writes must be one the hook script actually
# maps to a status, or the agent pays for a hook that reports nothing.
missing=''
for ev in $(jq -r '.hooks | keys[]' "$CLAUDE_JSON"); do
  grep -q "$ev" "$ROOT/scripts/zj-agent-mob-hook.sh" || missing="$missing $ev"
done
assert_eq "every claude event is handled by the hook" "$missing" ""
# async so a slow hook can never stall a turn; this is the whole reason the
# plugin is safe to leave installed. PermissionRequest is the sole exception:
# returning a decision requires blocking, which is why it is opt-in at runtime
# via ZJ_AGENT_APPROVE and falls through to the agent's own prompt on timeout.
assert_eq "claude reporting hooks are async" \
  "$(jq '[.hooks | to_entries[] | select(.key != "PermissionRequest")
         | .value[].hooks[] | select(.async != true)] | length' "$CLAUDE_JSON")" "0"
assert_eq "PermissionRequest is the only synchronous hook" \
  "$(jq '[.hooks | to_entries[] | select(.value[].hooks[].async != true) | .key] | join(",")' \
     "$CLAUDE_JSON")" '"PermissionRequest"'
assert_eq "claude hooks are command type" \
  "$(jq '[.hooks[][].hooks[] | select(.type != "command")] | length' "$CLAUDE_JSON")" "0"
# Notification fires for many things; only the two that mean "needs you" should
# flip the agent to waiting.
assert_eq "Notification is scoped to prompts" \
  "$(jq -r '.hooks.Notification[0].matcher' "$CLAUDE_JSON")" \
  "permission_prompt|idle_prompt"
assert_eq "settings.json is valid json" \
  "$(jq -e . "$CLAUDE_JSON" >/dev/null 2>&1 && echo yes || echo no)" "yes"

echo
echo "codex hook contract"
fresh
init install codex || true
for ev in SessionStart UserPromptSubmit PreToolUse PostToolUse PermissionRequest Stop SessionEnd; do
  assert_contains "codex registers $ev" "$(cmds_for "$CODEX_JSON" "$ev")" "$HOOK_CMD"
done
missing=''
for ev in $(jq -r '.hooks | keys[]' "$CODEX_JSON"); do
  grep -q "$ev" "$ROOT/scripts/zj-agent-mob-hook.sh" || missing="$missing $ev"
done
assert_eq "every codex event is handled by the hook" "$missing" ""
# Without ZJ_AGENT_TOOL=codex the hook reads a Claude transcript that does not
# exist and every codex agent shows an empty task.
assert_eq "codex commands set ZJ_AGENT_TOOL=codex" \
  "$(jq '[.hooks[][].hooks[] | select(.command | startswith("env ZJ_AGENT_TOOL=codex ") | not)] | length' \
      "$CODEX_JSON")" "0"
# Codex has no async flag; emitting one risks a schema rejection.
assert_eq "codex hooks carry no async flag" \
  "$(jq '[.hooks[][].hooks[] | select(has("async"))] | length' "$CODEX_JSON")" "0"

echo
echo "the two targets are independent"
fresh
init install claude || true
assert_contains "installing claude leaves codex alone" \
  "$(sh "$INIT" status | tr '\n' ' ')" "codex=absent"
init install codex || true
status=$(sh "$INIT" status | tr '\n' ' ')
assert_contains "both report installed" "$status" "claude=installed"
assert_contains "both report installed (codex)" "$status" "codex=installed"
init uninstall claude || true
status=$(sh "$INIT" status | tr '\n' ' ')
assert_contains "uninstalling claude spares codex" "$status" "codex=installed"
assert_contains "uninstalling claude removes claude" "$status" "claude=absent"

fresh
init install claude codex || true
status=$(sh "$INIT" status | tr '\n' ' ')
assert_contains "one run installs several targets" "$status" "claude=installed"
assert_contains "one run installs several targets (codex)" "$status" "codex=installed"

echo
echo "status output the plugin parses"
fresh
# The install screen splits on `=` and matches these exact keys and states.
out=$(sh "$INIT" status)
assert_eq "status prints four keys" "$(printf '%s\n' "$out" | wc -l | tr -d ' ')" "4"
for k in claude codex plugin hook; do
  assert_contains "status reports $k" "$out" "$k="
done
bogus=$(printf '%s\n' "$out" | grep -cv '^[a-z]*=\(installed\|absent\)$' || true)
assert_eq "every status line is key=installed|absent" "$bogus" "0"

echo
echo "installing without a source tree"
# The curl-pipe install: no repo beside the script, everything fetched. A
# `file://` release URL keeps this offline and deterministic while still
# exercising the real download path (curl and wget both speak file://).
RELEASE_SRV="$WORK/release"
mkdir -p "$RELEASE_SRV"
cp "$ROOT/scripts/zj-agent-mob-hook.sh" "$RELEASE_SRV/"
cp "$INIT" "$RELEASE_SRV/init.sh"
printf 'fake wasm with _start export\n' > "$RELEASE_SRV/zj-agent-mob.wasm"
export ZJ_AGENT_RELEASE_URL="file://$RELEASE_SRV"

if command -v curl >/dev/null 2>&1 || command -v wget >/dev/null 2>&1; then
  fresh
  # Bare `sh < file` leaves $0 as "sh" and stdin consumed, which is exactly the
  # shape of `curl ... | sh` and the case the self-copy fallback exists for.
  ISOLATED="$WORK/isolated"
  mkdir -p "$ISOLATED"
  cp "$INIT" "$ISOLATED/init.sh"
  ( cd "$ISOLATED" && sh < "$ISOLATED/init.sh" ) >"$WORK/out" 2>"$WORK/err" || true
  status=$(sh "$INIT" status | tr '\n' ' ')
  assert_contains "piped install hooks claude" "$status" "claude=installed"
  assert_contains "piped install hooks codex" "$status" "codex=installed"
  assert_contains "piped install fetches the plugin" "$status" "plugin=installed"
  # The whole point: the install screen has an installer to drive afterwards.
  assert_contains "piped install leaves a hook" "$status" "hook=installed"
  if [ -x "$ZJ_AGENT_HOOK_DIR/install.sh" ]; then ok "piped install self-installs install.sh"; else
    bad "piped install self-installs install.sh" "missing; install screen would report none"; fi

  fresh
  # --from-release must prefer the release even when a local build exists.
  init install plugin --from-release || true
  assert_contains "--from-release installs the plugin" \
    "$(sh "$INIT" status | tr '\n' ' ')" "plugin=installed"

  fresh
  # ZJ_AGENT_RELEASE_URL pins the whole URL, so it has to go for --version to
  # be the thing under test. Point at a directory that does not exist rather
  # than a bad tag on github, which would make this case need the network.
  rc=0
  ( unset ZJ_AGENT_RELEASE_URL
    ZJ_AGENT_RELEASE_URL="file://$WORK/no-such-release" \
    sh "$INIT" install plugin --version v0.0.0-nope >/dev/null 2>&1 ) || rc=$?
  assert_eq "an unresolvable version fails loudly" "$rc" "1"
else
  echo "  SKIP: neither curl nor wget available"
fi
unset ZJ_AGENT_RELEASE_URL

fresh
rc=0; sh "$INIT" --version >/dev/null 2>&1 || rc=$?
assert_eq "--version without a value is rejected" "$rc" "1"

echo
echo "idempotence"
fresh
init install claude codex || true
c1=$(jq '[.hooks[][].hooks[]] | length' "$CLAUDE_JSON")
x1=$(jq '[.hooks[][].hooks[]] | length' "$CODEX_JSON")
init install claude codex || true
init install claude codex || true
assert_eq "claude re-install does not duplicate" \
  "$(jq '[.hooks[][].hooks[]] | length' "$CLAUDE_JSON")" "$c1"
assert_eq "codex re-install does not duplicate" \
  "$(jq '[.hooks[][].hooks[]] | length' "$CODEX_JSON")" "$x1"

echo
echo "existing user config survives a round trip"
fresh
mkdir -p "$CLAUDE_CONFIG_DIR" "$CODEX_HOME"
# A realistic settings.json: unrelated top-level keys plus a user's own hook on
# an event we also use. Clobbering either would be a destructive install.
cat > "$CLAUDE_JSON" <<'JSON'
{
  "model": "opus",
  "permissions": {"allow": ["Bash(git status:*)"]},
  "hooks": {
    "Stop": [{"hooks": [{"type": "command", "command": "/usr/local/bin/my-notify"}]}],
    "PreCompact": [{"matcher": "*", "hooks": [{"type": "command", "command": "/usr/local/bin/archive"}]}]
  }
}
JSON
cat > "$CODEX_JSON" <<'JSON'
{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "/usr/local/bin/my-codex-notify"}]}]}}
JSON
before_claude=$(jq -S . "$CLAUDE_JSON")
before_codex=$(jq -S . "$CODEX_JSON")

init install claude codex || true
assert_eq "unrelated settings keys are preserved" \
  "$(jq -r '.model + " " + .permissions.allow[0]' "$CLAUDE_JSON")" \
  "opus Bash(git status:*)"
assert_contains "a user's own hook on a shared event survives" \
  "$(cmds_for "$CLAUDE_JSON" Stop)" "/usr/local/bin/my-notify"
assert_contains "our hook is added alongside it" \
  "$(cmds_for "$CLAUDE_JSON" Stop)" "$HOOK_CMD"
assert_contains "an event we never touch is untouched" \
  "$(cmds_for "$CLAUDE_JSON" PreCompact)" "/usr/local/bin/archive"
assert_contains "codex user hooks survive install" \
  "$(cmds_for "$CODEX_JSON" Stop)" "/usr/local/bin/my-codex-notify"

# The real test of a safe uninstall: byte-for-byte back to where we started.
init uninstall claude codex || true
assert_eq "claude uninstall restores the original settings" \
  "$(jq -S . "$CLAUDE_JSON")" "$before_claude"
assert_eq "codex uninstall restores the original hooks" \
  "$(jq -S . "$CODEX_JSON")" "$before_codex"

# Uninstalling from a file that only ever had our hooks must not leave a husk
# of empty objects behind.
fresh
init install claude || true
init uninstall claude || true
assert_eq "a hooks-only settings.json is left clean" \
  "$(jq -Sc . "$CLAUDE_JSON")" "{}"

echo
echo "dry run writes nothing"
fresh
init install claude codex --dry-run || true
if [ -e "$CLAUDE_JSON" ] || [ -e "$CODEX_JSON" ] || [ -e "$HOOK_CMD" ]; then
  bad "dry run creates no files" "something was written"
else
  ok "dry run creates no files"
fi
assert_contains "dry run says what it would do" \
  "$(sh "$INIT" install claude --dry-run)" "dry-run"

fresh
init install claude || true
snapshot=$(jq -S . "$CLAUDE_JSON")
init install claude --dry-run || true
assert_eq "dry run does not modify an existing install" \
  "$(jq -S . "$CLAUDE_JSON")" "$snapshot"
init uninstall --dry-run || true
assert_eq "dry-run uninstall leaves the install in place" \
  "$(jq -S . "$CLAUDE_JSON")" "$snapshot"
if [ -x "$HOOK_CMD" ]; then ok "dry-run uninstall keeps hook.sh"; else
  bad "dry-run uninstall keeps hook.sh" "hook.sh was removed"; fi

echo
echo "symlinked settings (stow/dotfiles) are written through"
fresh
mkdir -p "$SANDBOX/dotfiles" "$CLAUDE_CONFIG_DIR"
echo '{"model":"opus"}' > "$SANDBOX/dotfiles/settings.json"
ln -s "$SANDBOX/dotfiles/settings.json" "$CLAUDE_JSON"
init install claude || true
if [ -L "$CLAUDE_JSON" ]; then ok "the symlink is not replaced by a file"; else
  bad "the symlink is not replaced by a file" "settings.json is now a regular file"; fi
assert_contains "the hook lands in the real dotfiles file" \
  "$(cmds_for "$SANDBOX/dotfiles/settings.json" Stop)" "$HOOK_CMD"
assert_contains "the symlink is called out in the output" \
  "$(sh "$INIT" install claude 2>&1)" "symlink"

echo
echo "backups"
fresh
mkdir -p "$CLAUDE_CONFIG_DIR"
echo '{"model":"opus"}' > "$CLAUDE_JSON"
init install claude || true
n=$(find "$CLAUDE_CONFIG_DIR" -name 'settings.json.bak-*' | wc -l | tr -d ' ')
if [ "$n" -ge 1 ]; then ok "an existing settings.json is backed up"; else
  bad "an existing settings.json is backed up" "no .bak-* file"; fi
bak=$(find "$CLAUDE_CONFIG_DIR" -name 'settings.json.bak-*' | head -1)
assert_eq "the backup holds the pre-install content" \
  "$(jq -Sc . "$bak")" '{"model":"opus"}'

echo
echo "plugin target"
# These cases turn on whether a built wasm exists beside the installer, so they
# run against a copy of the source tree rather than the repo: otherwise they
# would pass or fail depending on whether someone had run cargo build.
UNBUILT="$WORK/unbuilt"
mkdir -p "$UNBUILT/scripts"
cp "$INIT" "$UNBUILT/init.sh"
cp "$ROOT/scripts/zj-agent-mob-hook.sh" "$UNBUILT/scripts/"

fresh
# --no-download keeps these offline. Without it an unbuilt tree now falls back
# to fetching the release, so these cases would depend on the network and
# quietly assert nothing when it is unavailable.
sh "$UNBUILT/init.sh" install --no-download >/dev/null 2>&1 || true
assert_contains "a missing wasm only warns during a full install" \
  "$(sh "$UNBUILT/init.sh" install --no-download 2>&1 || true)" "plugin not built"
assert_contains "the agents are installed anyway" \
  "$(sh "$INIT" status | tr '\n' ' ')" "claude=installed"
# Asking for the plugin alone is explicit, so a missing build is a hard error.
rc=0; sh "$UNBUILT/init.sh" install plugin --no-download >/dev/null 2>&1 || rc=$?
assert_eq "install plugin alone fails when unbuilt" "$rc" "1"

fresh
mkdir -p "$WORK/wasmsrc/target/wasm32-wasip1/release"
cp "$INIT" "$WORK/wasmsrc/init.sh"
mkdir -p "$WORK/wasmsrc/scripts"
cp "$ROOT/scripts/zj-agent-mob-hook.sh" "$WORK/wasmsrc/scripts/"
printf 'fake wasm with _start export\n' \
  > "$WORK/wasmsrc/target/wasm32-wasip1/release/zj-agent-mob.wasm"
sh "$WORK/wasmsrc/init.sh" install plugin >/dev/null 2>&1 || true
if [ -f "$ZJ_AGENT_PLUGIN_DIR/zj-agent-mob.wasm" ]; then
  ok "a built wasm is copied to the plugin dir"
else
  bad "a built wasm is copied to the plugin dir" "nothing at $ZJ_AGENT_PLUGIN_DIR"
fi
assert_contains "plugin=installed once copied" \
  "$(sh "$INIT" status | tr '\n' ' ')" "plugin=installed"
# Zellij's loader needs the WASI `_start` export; a cdylib-only build has none
# and fails at load with "could not find exported function".
printf 'no start symbol here\n' \
  > "$WORK/wasmsrc/target/wasm32-wasip1/release/zj-agent-mob.wasm"
assert_contains "a wasm without _start warns" \
  "$(sh "$WORK/wasmsrc/init.sh" install plugin 2>&1 || true)" "no _start export"
init uninstall plugin || true
assert_contains "uninstall plugin removes the wasm" \
  "$(sh "$INIT" status | tr '\n' ' ')" "plugin=absent"

echo
echo "installing from the self-copy, with no repo present"
fresh
init install claude || true
# This is the plugin's path: it runs ~/.config/zj-agent-mob/install.sh, which
# has no scripts/ dir beside it and must fall back to the installed hook.sh.
rc=0
sh "$ZJ_AGENT_HOOK_DIR/install.sh" install codex >/dev/null 2>&1 || rc=$?
assert_eq "the self-copy installs without the source tree" "$rc" "0"
assert_contains "codex is installed by the self-copy" \
  "$(sh "$INIT" status | tr '\n' ' ')" "codex=installed"
rc=0
sh "$ZJ_AGENT_HOOK_DIR/install.sh" status >/dev/null 2>&1 || rc=$?
assert_eq "the self-copy reports status" "$rc" "0"

echo
echo "uninstall is safe when nothing is installed"
fresh
rc=0; sh "$INIT" uninstall >/dev/null 2>&1 || rc=$?
assert_eq "uninstall on a clean machine exits 0" "$rc" "0"
assert_contains "status is still parseable afterwards" \
  "$(sh "$INIT" status | tr '\n' ' ')" "claude=absent"

echo
echo "the installed hook actually reports for both agents"
# Closes the loop: install for real, then run the installed hook.sh the way each
# agent would and check the pipe args. Anything wrong in the wiring shows here.
fresh
init install claude codex || true
mkdir -p "$WORK/bin"
cat > "$WORK/bin/zellij" <<'STUB'
#!/bin/sh
printf '%s\n' "$*" >> "$ZJ_TEST_CAPTURE"
STUB
chmod +x "$WORK/bin/zellij"

# drive <event-json> [env...] -> what the installed hook sent to zellij
drive() {
  _json=$1; shift
  ZJ_TEST_CAPTURE="$WORK/drive.out"; : > "$ZJ_TEST_CAPTURE"
  printf '%s' "$_json" | env PATH="$WORK/bin:$PATH" ZELLIJ_PANE_ID=7 \
    ZJ_TEST_CAPTURE="$ZJ_TEST_CAPTURE" ZJ_AGENT_PLUGIN=file:/p.wasm "$@" \
    sh "$HOOK_CMD" >/dev/null 2>&1 || true
  cat "$ZJ_TEST_CAPTURE"
}

out=$(drive '{"hook_event_name":"Stop","session_id":"s1","cwd":"/w/api"}')
assert_contains "installed hook reports for claude" "$out" "status=done"
assert_contains "claude reports tool=claude" "$out" "tool=claude"

# Exactly the command Codex is configured to run, env prefix and all.
out=$(drive '{"hook_event_name":"PermissionRequest"}' ZJ_AGENT_TOOL=codex)
assert_contains "installed hook reports for codex" "$out" "status=waiting"
assert_contains "codex reports tool=codex" "$out" "tool=codex"

# The command string in hooks.json must be runnable as written.
codex_cmd=$(jq -r '.hooks.Stop[0].hooks[0].command' "$CODEX_JSON")
ZJ_TEST_CAPTURE="$WORK/drive.out"; : > "$ZJ_TEST_CAPTURE"
printf '{"hook_event_name":"Stop"}' | env PATH="$WORK/bin:$PATH" ZELLIJ_PANE_ID=7 \
  ZJ_TEST_CAPTURE="$ZJ_TEST_CAPTURE" ZJ_AGENT_PLUGIN=file:/p.wasm \
  sh -c "$codex_cmd" >/dev/null 2>&1 || true
assert_contains "the codex command string runs as configured" \
  "$(cat "$WORK/drive.out")" "tool=codex"

claude_cmd=$(jq -r '.hooks.Stop[0].hooks[0].command' "$CLAUDE_JSON")
ZJ_TEST_CAPTURE="$WORK/drive.out"; : > "$ZJ_TEST_CAPTURE"
printf '{"hook_event_name":"Stop"}' | env PATH="$WORK/bin:$PATH" ZELLIJ_PANE_ID=7 \
  ZJ_TEST_CAPTURE="$ZJ_TEST_CAPTURE" ZJ_AGENT_PLUGIN=file:/p.wasm \
  sh -c "$claude_cmd" >/dev/null 2>&1 || true
assert_contains "the claude command string runs as configured" \
  "$(cat "$WORK/drive.out")" "tool=claude"

echo
echo "settings files stay \$HOME-relative"
# These configs get committed to dotfiles repos, so the hook path must not bake
# in a username. Everything above runs with the install dirs outside $HOME, which
# exercises the absolute branch; here $HOME contains them so the rewrite applies.
CASE=$((CASE + 1))
FAKE_HOME="$WORK/case$CASE-home"
mkdir -p "$FAKE_HOME"
# Deliberately unset the dir overrides so the installer derives them from $HOME.
home_init() {
  env -u ZJ_AGENT_HOOK_DIR -u ZJ_AGENT_PLUGIN_DIR -u CLAUDE_CONFIG_DIR -u CODEX_HOME \
    HOME="$FAKE_HOME" sh "$INIT" "$@"
}
HOME_CLAUDE="$FAKE_HOME/.claude/settings.json"
HOME_CODEX="$FAKE_HOME/.codex/hooks.json"
HOME_ABS="$FAKE_HOME/.config/zj-agent-mob/hook.sh"

home_init install claude codex >/dev/null 2>&1 || true
# shellcheck disable=SC2016  # The literal, unexpanded `$HOME` is what we assert.
assert_eq "claude hook is written \$HOME-relative" \
  "$(jq -r '.hooks.Stop[0].hooks[0].command' "$HOME_CLAUDE")" \
  '$HOME/.config/zj-agent-mob/hook.sh'
# shellcheck disable=SC2016  # The literal, unexpanded `$HOME` is what we assert.
assert_eq "codex hook is written \$HOME-relative" \
  "$(jq -r '.hooks.Stop[0].hooks[0].command' "$HOME_CODEX")" \
  'env ZJ_AGENT_TOOL=codex $HOME/.config/zj-agent-mob/hook.sh'
# The whole point: no absolute home path anywhere in a file people commit.
if grep -q "$FAKE_HOME" "$HOME_CLAUDE" "$HOME_CODEX"; then
  bad "settings leak no absolute home path" "found $FAKE_HOME in a settings file"
else
  ok "settings leak no absolute home path"
fi
assert_contains "status recognizes the \$HOME form" \
  "$(home_init status | tr '\n' ' ')" "claude=installed"

# Re-running must not double-register: the filter has to match what it wrote.
home_init install claude codex >/dev/null 2>&1 || true
assert_eq "re-install does not duplicate the \$HOME form" \
  "$(jq '[.hooks.Stop[].hooks[]] | length' "$HOME_CLAUDE")" "1"

# A $HOME-relative command is only useful if the shell can actually run it.
ZJ_TEST_CAPTURE="$WORK/drive.out"; : > "$ZJ_TEST_CAPTURE"
home_cmd=$(jq -r '.hooks.Stop[0].hooks[0].command' "$HOME_CODEX")
printf '{"hook_event_name":"Stop"}' | env PATH="$WORK/bin:$PATH" ZELLIJ_PANE_ID=7 \
  HOME="$FAKE_HOME" ZJ_TEST_CAPTURE="$ZJ_TEST_CAPTURE" ZJ_AGENT_PLUGIN=file:/p.wasm \
  sh -c "$home_cmd" >/dev/null 2>&1 || true
assert_contains "the \$HOME command string runs once expanded" \
  "$(cat "$WORK/drive.out")" "tool=codex"

home_init uninstall >/dev/null 2>&1 || true
assert_contains "uninstall clears the \$HOME form" \
  "$(home_init status | tr '\n' ' ')" "claude=absent"

echo
echo "installs predating the \$HOME rewrite still migrate"
# Entries written by the older installer are absolute. They must still be
# recognized, replaced rather than duplicated, and removable.
CASE=$((CASE + 1))
FAKE_HOME="$WORK/case$CASE-home"
mkdir -p "$FAKE_HOME/.claude" "$FAKE_HOME/.codex"
HOME_CLAUDE="$FAKE_HOME/.claude/settings.json"
HOME_CODEX="$FAKE_HOME/.codex/hooks.json"
HOME_ABS="$FAKE_HOME/.config/zj-agent-mob/hook.sh"
# A legacy install, plus an unrelated user hook that must survive untouched.
jq -n --arg c "$HOME_ABS" '{hooks:{
  Stop:[{hooks:[{type:"command",command:$c,async:true}]}],
  PreToolUse:[{matcher:"*",hooks:[{type:"command",command:"echo mine"}]}]}}' > "$HOME_CLAUDE"
jq -n --arg c "env ZJ_AGENT_TOOL=codex $HOME_ABS" \
  '{hooks:{Stop:[{hooks:[{type:"command",command:$c}]}]}}' > "$HOME_CODEX"

assert_contains "status recognizes a legacy absolute install" \
  "$(home_init status | tr '\n' ' ')" "claude=installed"
home_init install claude codex >/dev/null 2>&1 || true
# shellcheck disable=SC2016  # The literal, unexpanded `$HOME` is what we assert.
assert_eq "legacy claude entry is replaced, not duplicated" \
  "$(jq -c '[.hooks.Stop[].hooks[].command]' "$HOME_CLAUDE")" \
  '["$HOME/.config/zj-agent-mob/hook.sh"]'
# shellcheck disable=SC2016  # The literal, unexpanded `$HOME` is what we assert.
assert_eq "legacy codex entry is replaced, not duplicated" \
  "$(jq -c '[.hooks.Stop[].hooks[].command]' "$HOME_CODEX")" \
  '["env ZJ_AGENT_TOOL=codex $HOME/.config/zj-agent-mob/hook.sh"]'
assert_contains "an unrelated user hook survives migration" \
  "$(jq -c '[.hooks.PreToolUse[].hooks[].command]' "$HOME_CLAUDE")" '"echo mine"'

# And uninstall must clean up a legacy install it never wrote itself.
jq -n --arg c "$HOME_ABS" '{hooks:{Stop:[{hooks:[{type:"command",command:$c}]}]}}' > "$HOME_CLAUDE"
home_init uninstall >/dev/null 2>&1 || true
assert_eq "uninstall removes a legacy absolute entry" \
  "$(jq -r '.hooks // "removed"' "$HOME_CLAUDE")" "removed"

echo
echo "-------------------------------------------"
printf '%d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
