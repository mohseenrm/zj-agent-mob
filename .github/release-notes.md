Fixes panels stacking up until a session fills with nested `Agent Mob` panes. One session hit 74 copies.

Zellij saves floating plugin panes into the session layout. Resurrecting restored every saved copy, then the keybind launched another beside them, so each cycle left one more panel behind.

## Changes

- The panel now closes itself when an older copy is already running. Any stack collapses to one.
- Removed the `Interrupt` hook. Claude Code has no such event, so it never fired. Interrupted turns still show as `idlewait`.

## Upgrading

```sh
curl -fsSL https://github.com/mohseenrm/zj-agent-mob/releases/download/v0.10.3/init.sh | sh
```

Reload the plugin, since Zellij caches compiled builds:

```sh
zellij action launch-or-focus-plugin --skip-plugin-cache --floating \
  "file:$HOME/.config/zellij/plugins/zj-agent-mob.wasm"
```

The hook changed in this release, so restart your agents after installing.

Already have a stack? The new build clears it on load. A session saved beforehand may restore it once more; start it again and it settles.

**Full changelog:** https://github.com/mohseenrm/zj-agent-mob/compare/v0.10.2...v0.10.3
