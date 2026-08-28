#!/bin/sh
# Run the same checks CI does, in the same order, and stop where CI would.
#
# CI is two jobs across five steps documented in development.md. Copying them
# out of a doc by hand is how a push fails on the one you forgot, so this is the
# single command for the pre-push loop.
#
#   ./scripts/check.sh          everything
#   ./scripts/check.sh fast     skip the wasm build and the installer e2e
#   ./scripts/check.sh -l       list the steps without running them
set -eu

cd "$(dirname "$0")/.."

MODE="${1:-all}"
WASM=target/wasm32-wasip1/release/zj-agent-mob.wasm
EXPORTS="_start load update render pipe plugin_version"

# Each step is `name|when|command`. `fast` steps run in both modes.
steps() {
  cat <<'STEPS'
fmt|fast|cargo fmt --all --check
clippy|fast|cargo clippy --all-targets -- -D warnings
test|fast|cargo test --all-targets
shellcheck|fast|shellcheck --shell=sh init.sh scripts/zj-agent-mob-hook.sh scripts/check.sh tests/e2e-install.sh
wasm|all|cargo build --release --target wasm32-wasip1
exports|all|check_exports
installer|all|./tests/e2e-install.sh
STEPS
}

# Zellij's loader needs all six or it fails at load with "could not find
# exported function", which no other check here would catch.
check_exports() {
  command -v wasm-objdump >/dev/null 2>&1 || {
    echo "  skipped: wasm-objdump not installed (brew install wabt)"
    return 0
  }
  dump=$(wasm-objdump -x "$WASM")
  for sym in $EXPORTS; do
    printf '%s' "$dump" | grep -q "<$sym> -> \"$sym\"" || {
      echo "  missing export: $sym"
      return 1
    }
  done
  echo "  all six exports present"
}

if [ "$MODE" = "-l" ] || [ "$MODE" = "--list" ]; then
  printf '%s\n' "steps, in CI order:"
  cat <<'LIST'
  fmt         cargo fmt --all --check
  clippy      cargo clippy --all-targets -- -D warnings
  test        cargo test --all-targets              (unit + hook e2e)
  shellcheck  shellcheck the three shipped scripts
  wasm        cargo build --release --target wasm32-wasip1   [skipped by `fast`]
  exports     the six symbols Zellij loads                   [skipped by `fast`]
  installer   ./tests/e2e-install.sh                         [skipped by `fast`]
LIST
  exit 0
fi

case "$MODE" in
  all | fast) ;;
  *)
    echo "usage: $0 [all|fast|-l]" >&2
    exit 2
    ;;
esac

failed=''
ran=0
start=$(date +%s)

# The loop reads from a heredoc rather than a pipe so `failed` survives it.
while IFS='|' read -r name when cmd; do
  [ -n "$name" ] || continue
  if [ "$MODE" = fast ] && [ "$when" = all ]; then
    continue
  fi
  printf '\033[1m==> %s\033[0m\n' "$name"
  if eval "$cmd"; then
    ran=$((ran + 1))
  else
    # Keep going: one `cargo fmt` diff should not hide a failing test.
    failed="$failed $name"
  fi
done <<EOF
$(steps)
EOF

elapsed=$(($(date +%s) - start))

if [ -n "$failed" ]; then
  printf '\n\033[31mFAILED:\033[0m%s  (%d passed, %ds)\n' "$failed" "$ran" "$elapsed"
  exit 1
fi

printf '\n\033[32mall %d checks passed\033[0m  (%ds)\n' "$ran" "$elapsed"
