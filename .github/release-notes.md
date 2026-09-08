The controls used to sit directly below the last agent, so they moved whenever the list changed. They now stay at the bottom of the pane while agent rows remain at the top.

## The fix

- Bottom-aligned hints in the agent list, setup screen, and install screen.
- Prompts and errors stay in the same footer area.
- The tour now shows the new layout.
- Unit tests and the real-Zellij harness cover the spacing.

## Upgrading

```sh
curl -fsSL https://github.com/mohseenrm/zj-agent-mob/releases/download/v0.10.2/init.sh | sh
```

Then reload the plugin, since Zellij caches compiled plugins:

```sh
zellij action launch-or-focus-plugin --skip-plugin-cache --floating \
  "file:$HOME/.config/zellij/plugins/zj-agent-mob.wasm"
```

No hook changes in this release, so running agents do not need a restart.

**Full changelog:** https://github.com/mohseenrm/zj-agent-mob/compare/v0.10.1...v0.10.2
