## What

<!-- What changes, and why. Link an issue if there is one. -->

## How it was verified

<!-- Delete what doesn't apply. CI covers the first four; the rest need a real
     Zellij session, which CI cannot provide. -->

- [ ] `cargo test`
- [ ] `cargo clippy --all-targets`
- [ ] `cargo fmt --check`
- [ ] `shellcheck --shell=sh init.sh scripts/zj-agent-mob-hook.sh`
- [ ] Built the wasm and loaded it in a live Zellij session
- [ ] Exercised the panel by hand (jump, kill, dismiss, install screen)

## Checklist

- [ ] Rebuilt the plugin and reloaded with `--skip-plugin-cache` before testing
      (Zellij caches compiled plugins; without this a change looks like a no-op)
- [ ] `./init.sh --dry-run` still reports the expected changes
- [ ] `./init.sh uninstall` still removes exactly what was installed
- [ ] Panel rows stay within the pane width at 40, 60, and 110 columns
- [ ] README updated if behaviour, keys, or configuration changed

## Notes for the reviewer

<!-- Anything non-obvious: a tradeoff, a Zellij API quirk, a deliberate
     omission. Delete if there's nothing to add. -->
