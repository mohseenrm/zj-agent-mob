# Troubleshooting

- [Changes to the plugin seem to have no effect](#changes-to-the-plugin-seem-to-have-no-effect)
- [The plugin wasm in my dotfiles repo is out of date](#the-plugin-wasm-in-my-dotfiles-repo-is-out-of-date)
- [The panel says "no agents in this session"](#the-panel-says-no-agents-in-this-session)
- [The install screen says "Installer not found"](#the-install-screen-says-installer-not-found)
- [The install screen shows `?` / "unknown" for everything](#the-install-screen-shows---unknown-for-everything)
- [Zellij fails to load the plugin](#zellij-fails-to-load-the-plugin)
- [`waiting` stays on screen after you've answered](#waiting-stays-on-screen-after-youve-answered)
- [Hooks landed in my dotfiles repo](#hooks-landed-in-my-dotfiles-repo)
- [The panel is cramped or columns are missing](#the-panel-is-cramped-or-columns-are-missing)

## Changes to the plugin seem to have no effect

**Zellij caches compiled plugins, in two places.** Zellij compiles the wasm to native code and caches it keyed by the plugin's *file path*. That path is identical on every rebuild, so the cache key never changes and a rebuilt plugin looks unchanged:

1. **On disk** — the compiled artifact, under `~/Library/Caches/org.Zellij-Contributors.Zellij/file:<path-to>/zj-agent-mob.wasm/` (macOS) or `~/.cache/zellij/` (Linux).
2. **In memory** — the Zellij *server* keeps already-instantiated plugins for the lifetime of the session.

Clearing only the first is not enough, which is the trap: `--skip-plugin-cache` bypasses the on-disk cache **only when a new plugin instance is created**. If the session already has one loaded, `launch-or-focus-plugin` focuses the existing instance rather than building a new one, so the flag silently does nothing. Closing the plugin pane does not help either — the server still holds the compiled module.

**The reliable fix is a brand-new session**, which means a new server process:

```sh
# from a terminal OUTSIDE zellij
rm -rf ~/Library/Caches/org.Zellij-Contributors.Zellij/   # macOS
# rm -rf ~/.cache/zellij/                                 # Linux
zellij --session fresh
```

Existing sessions are unaffected by the cache removal; they keep running their loaded copy.

> [!NOTE]
> Removing the whole cache directory also drops `permissions.kdl` and `session_info/`, so the plugin re-asks for permissions on next load and other sessions lose resurrection metadata. To clear just this plugin, delete only its own entries (note the literal `file:` directory in the path):
>
> ```sh
> C=~/Library/Caches/org.Zellij-Contributors.Zellij
> rm -rf "$C"/*/"file:$HOME/.config/zellij/plugins/zj-agent-mob.wasm" \
>        "$C/file:$HOME/.config/zellij/plugins/zj-agent-mob.wasm"
> ```

Within an existing session, `--skip-plugin-cache` does work if no instance is loaded yet (note it lives on `launch-or-focus-plugin`; `zellij pipe` has no such flag):

```sh
zellij action launch-or-focus-plugin --skip-plugin-cache --floating \
  "file:$HOME/.config/zellij/plugins/zj-agent-mob.wasm"
```

**How to tell which build you are looking at.** Compare the installed artifact against what you just built — if these differ, the problem is the install step, not the cache:

```sh
md5 -q target/wasm32-wasip1/release/zj-agent-mob.wasm \
       ~/.config/zellij/plugins/zj-agent-mob.wasm
```

If they match but the panel still looks stale, it is the cache, and you need a new session.

## The plugin wasm in my dotfiles repo is out of date

Unlike the hook settings files, the plugin wasm is **not** written through a symlink. `init.sh` copies it to `~/.config/zellij/plugins/`, and if `stow` created that as a real directory rather than a symlink, the copy in your dotfiles repo is a separate file that never gets updated. Re-run `stow --adopt` from the dotfiles repo, or copy it across manually:

```sh
cp ~/.config/zellij/plugins/zj-agent-mob.wasm \
   ~/dotfiles/.config/zellij/plugins/zj-agent-mob.wasm
```

Check which situation you are in with `readlink ~/.config/zellij/plugins/zj-agent-mob.wasm` — no output means it is a regular file, so the two copies can drift.

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
