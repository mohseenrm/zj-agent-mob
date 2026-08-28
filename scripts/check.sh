#!/bin/sh
# The checks, defined once. CI runs them step by step; you run them all at once.
#
# Every command CI executes lives here rather than in ci.yml, so the two cannot
# drift: the workflow calls `./scripts/check.sh <step>` per step, which keeps
# GitHub's per-step log sections and parallel jobs while this file stays the
# single source of truth.
#
#   ./scripts/check.sh          everything
#   ./scripts/check.sh fast     skip the wasm build, exports and installer e2e
#   ./scripts/check.sh test     one step by name (what CI calls)
#   ./scripts/check.sh -l       list the steps
set -eu

cd "$(dirname "$0")/.."

WASM=target/wasm32-wasip1/release/zj-agent-mob.wasm
EXPORTS="_start load update render pipe plugin_version"
SHELL_SCRIPTS="init.sh scripts/zj-agent-mob-hook.sh scripts/check.sh scripts/reinstall-local.sh tests/e2e-install.sh"

# `fast` steps are the ones quick enough for a tight local loop; `all` steps
# also run on a bare `./scripts/check.sh`.
STEPS="fmt clippy test shellcheck wasm exports installer"
FAST_STEPS="fmt clippy test shellcheck"

run_step() {
  case "$1" in
    fmt) cargo fmt --all --check ;;
    clippy) cargo clippy --all-targets -- -D warnings ;;
    # Tests run natively: host calls are stubbed off-wasm precisely so this works.
    test) cargo test --all-targets ;;
    shellcheck) run_shellcheck ;;
    wasm) cargo build --release --target wasm32-wasip1 ;;
    exports) check_exports ;;
    # The installer is the only supported path to a working install, and what it
    # writes is read by Claude Code and Codex themselves: a wrong event name or
    # matcher silences every agent and no Rust test would notice.
    installer) ./tests/e2e-install.sh ;;
    *)
      echo "unknown step: $1" >&2
      echo "steps: $STEPS" >&2
      return 2
      ;;
  esac
}

# Word splitting on the script list is intended, hence the disable.
run_shellcheck() {
  # shellcheck disable=SC2086
  shellcheck --shell=sh $SHELL_SCRIPTS
}

# Zellij's loader needs the WASI `_start` export, which only a bin target
# provides. A cdylib-only build fails at load with "could not find exported
# function", which no other check here would catch.
check_exports() {
  if ! command -v wasm-objdump >/dev/null 2>&1; then
    echo "skipped: wasm-objdump not installed (brew install wabt)"
    return 0
  fi
  dump=$(wasm-objdump -x "$WASM")
  missing=''
  for sym in $EXPORTS; do
    printf '%s' "$dump" | grep -q "<$sym> -> \"$sym\"" || missing="$missing $sym"
  done
  if [ -n "$missing" ]; then
    # ::error:: is a GitHub annotation; harmless noise in a local run.
    echo "::error::wasm is missing the$missing export(s)"
    return 1
  fi
  echo "all required exports present"
}

usage() {
  cat <<EOF
usage: $0 [all|fast|-l|<step>]

steps, in CI order:
  fmt         cargo fmt --all --check
  clippy      cargo clippy --all-targets -- -D warnings
  test        cargo test --all-targets              (unit + hook e2e)
  shellcheck  shellcheck the shipped scripts
  wasm        cargo build --release --target wasm32-wasip1   [skipped by \`fast\`]
  exports     the six symbols Zellij loads                   [skipped by \`fast\`]
  installer   ./tests/e2e-install.sh                         [skipped by \`fast\`]
EOF
}

MODE="${1:-all}"

case "$MODE" in
  -l | --list | -h | --help)
    usage
    exit 0
    ;;
  all) selected="$STEPS" ;;
  fast) selected="$FAST_STEPS" ;;
  *)
    # A single step: run it directly so CI gets its exit code and nothing else.
    run_step "$MODE"
    exit $?
    ;;
esac

failed=''
ran=0
start=$(date +%s)

for name in $selected; do
  printf '\033[1m==> %s\033[0m\n' "$name"
  # Keep going: a `cargo fmt` diff should not hide a failing test.
  if run_step "$name"; then
    ran=$((ran + 1))
  else
    failed="$failed $name"
  fi
done

elapsed=$(($(date +%s) - start))

if [ -n "$failed" ]; then
  printf '\n\033[31mFAILED:\033[0m%s  (%d passed, %ds)\n' "$failed" "$ran" "$elapsed"
  exit 1
fi

printf '\n\033[32mall %d checks passed\033[0m  (%ds)\n' "$ran" "$elapsed"
