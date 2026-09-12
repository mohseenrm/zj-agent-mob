Fixes a panel that could spin on load, filling the pane with stacked `Loading … zj-agent-mob.wasm` frames and leaving hook messages unanswered.

## What went wrong

When a second copy of the panel is open, the newer one closes itself so only one survives. But `close_self` is a request, not an instant exit: the pane keeps receiving updates until the host acts on it. The panel asked again on every one of those updates, dozens of times a second, which starved its own thread. Hook pipes then timed out waiting for it:

```
ERROR zellij_server::route: Action CliPipe did not complete within 1s timeout
```

Upgrading in place was the usual way to hit this, since reloading the plugin briefly leaves two copies alive.

## Changes

- The panel asks to close once, then waits. If the older copy disappears first, it unlatches and goes back to rendering.

## Upgrading

```sh
curl -fsSL https://github.com/mohseenrm/zj-agent-mob/releases/download/v0.11.1/init.sh | sh
```

Reload the plugin, since Zellij caches compiled builds:

```sh
zellij action launch-or-focus-plugin --skip-plugin-cache --floating \
  "file:$HOME/.config/zellij/plugins/zj-agent-mob.wasm"
```

The hook is unchanged from v0.11.0, so agents started since then don't need restarting.

**Full changelog:** https://github.com/mohseenrm/zj-agent-mob/compare/v0.11.0...v0.11.1
