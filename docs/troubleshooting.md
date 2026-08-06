# Troubleshooting

- [Changes to the plugin seem to have no effect](#changes-to-the-plugin-seem-to-have-no-effect)
- [The panel says "no agents in this session"](#the-panel-says-no-agents-in-this-session)
- [The install screen says "Installer not found"](#the-install-screen-says-installer-not-found)
- [The install screen shows `?` / "unknown" for everything](#the-install-screen-shows---unknown-for-everything)
- [Zellij fails to load the plugin](#zellij-fails-to-load-the-plugin)
- [`waiting` stays on screen after you've answered](#waiting-stays-on-screen-after-youve-answered)
- [Hooks landed in my dotfiles repo](#hooks-landed-in-my-dotfiles-repo)
- [The panel is cramped or columns are missing](#the-panel-is-cramped-or-columns-are-missing)

## Changes to the plugin seem to have no effect

**Zellij caches compiled plugins.** After rebuilding, a running session keeps using the old WASM. Force a reload with `--skip-plugin-cache`, which lives on `launch-or-focus-plugin` (note: *not* on `zellij pipe`, which has no such flag):

```sh
zellij action launch-or-focus-plugin --skip-plugin-cache --floating \
  "file:$HOME/.config/zellij/plugins/zj-agent-mob.wasm"
```

Or clear the cache and start a new session:

```sh
# macOS
rm -rf ~/Library/Caches/org.Zellij-Contributors.Zellij/
# Linux
rm -rf ~/.cache/zellij/
```

## The panel says "no agents in this session"

Work through these in order:

1. **Are the hooks installed?** Press <kbd>i</kbd>, or run `./init.sh status`.
2. **Did you restart the agent?** Hooks are read at session start. An agent that was already running when you installed them reports nothing.
3. **Is the agent inside Zellij?** The hook exits immediately when `$ZELLIJ_PANE_ID` is unset. Check with `echo $ZELLIJ_PANE_ID` in the agent's pane.
4. **Is the hook firing at all?** Turn on logging and watch it:

   ```sh
   export ZJ_AGENT_DEBUG=1
   tail -f ~/.cache/zj-agent-mob/hook.log
   ```

   No lines means the agent isn't invoking the hook; lines but no panel update means the `zellij pipe` call is failing.

5. **Can you drive the panel by hand?** This bypasses the agent entirely:

   ```sh
   zellij pipe --name agent-status \
     --plugin file:~/.config/zellij/plugins/zj-agent-mob.wasm \
     --args "pane_id=$ZELLIJ_PANE_ID,tool=claude,status=waiting,task=manual test"
   ```

## The install screen says "Installer not found"

The plugin drives `~/.config/zj-agent-mob/install.sh`, which `init.sh` puts there by copying itself. Run `./init.sh` from the repo once to bootstrap it.

## The install screen shows `?` / "unknown" for everything

The status command failed. Common causes: `jq` isn't installed, or the plugin wasn't granted Zellij's "Run commands" permission. Press <kbd>r</kbd> to retry; a real error message is shown under the rows when there is one.

## Zellij fails to load the plugin

```text
could not find exported function
```

The wasm was built as a cdylib rather than a bin. Confirm the artifact you installed is `zj-agent-mob.wasm` (hyphens, from the bin target), not `zj_agent_mob.wasm` (underscores, the cdylib), then rebuild:

```sh
cargo build --release --target wasm32-wasip1
./init.sh install plugin
```

## `waiting` stays on screen after you've answered

Claude has no "permission granted" event, so `waiting` to `working` relies on the next tool-event heartbeat. If you set `ZJ_AGENT_HEARTBEAT=0`, `waiting` persists until the turn ends. That's the tradeoff for halving hook volume.

## Hooks landed in my dotfiles repo

That's intended. `init.sh` resolves symlinks and writes through to the real file, so a stow-managed `~/.claude/settings.json` gets the change in your dotfiles repo, where you can commit it. The installer prints a note when it detects this.

## The panel is cramped or columns are missing

The layout degrades by width: the project column is dropped under 50 columns, and the per-agent detail line needs at least 60 columns plus two rows per agent. Resize the floating pane, or set a larger `width` / `height` in the layout.
