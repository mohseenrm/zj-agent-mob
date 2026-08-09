# Troubleshooting

**Nothing showing up?** Start with [the panel says "no agents in this session"](#the-panel-says-no-agents-in-this-session).

- [Changes to the plugin seem to have no effect](#changes-to-the-plugin-seem-to-have-no-effect)
- [The plugin wasm in my dotfiles repo is out of date](#the-plugin-wasm-in-my-dotfiles-repo-is-out-of-date)
- [The panel says "no agents in this session"](#the-panel-says-no-agents-in-this-session)
- [An agent in another Zellij session shows `found` but never live status](#an-agent-in-another-zellij-session-shows-found-but-never-live-status)
- [Task text from other sessions is visible to other users](#task-text-from-other-sessions-is-visible-to-other-users)
- [<kbd>x</kbd> does nothing on an agent from another session](#x-does-nothing-on-an-agent-from-another-session)
- [A row says `unknown` / `(session exited)`](#a-row-says-unknown--session-exited)
- [The install screen says "Installer not found"](#the-install-screen-says-installer-not-found)
- [The install screen shows `?` / "unknown" for everything](#the-install-screen-shows---unknown-for-everything)
- [Zellij fails to load the plugin](#zellij-fails-to-load-the-plugin)
- [`waiting` stays on screen after you've answered](#waiting-stays-on-screen-after-youve-answered)
- [Approve / reject from the panel does nothing](#approve--reject-from-the-panel-does-nothing)
- [Hooks landed in my dotfiles repo](#hooks-landed-in-my-dotfiles-repo)
- [The panel is cramped or columns are missing](#the-panel-is-cramped-or-columns-are-missing)

## Changes to the plugin seem to have no effect

**Zellij caches compiled plugins, in two places.** Zellij compiles the wasm to native code and caches it keyed by the plugin's *file path*. That path is identical on every rebuild, so the cache key never changes and a rebuilt plugin looks unchanged:

1. **On disk**: the compiled artifact, under `~/Library/Caches/org.Zellij-Contributors.Zellij/file:<path-to>/zj-agent-mob.wasm/` (macOS) or `~/.cache/zellij/` (Linux).
2. **In memory**: the Zellij *server* keeps already-instantiated plugins for the lifetime of the session.

Clearing the first alone is not enough, which is the trap: `--skip-plugin-cache` bypasses the on-disk cache **only when a new plugin instance is created**. If the session already has one loaded, `launch-or-focus-plugin` focuses the existing instance rather than building a new one, so the flag silently does nothing. Closing the plugin pane does not help either: the server still holds the compiled module.

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

**How to tell which build you are looking at.** Compare the installed artifact against what you just built. If these differ, the problem is the install step, not the cache:

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

Check which situation you are in with `readlink ~/.config/zellij/plugins/zj-agent-mob.wasm`: no output means it is a regular file, so the two copies can drift.

## The panel says "no agents in this session"

**First, check whether the panel says `found` instead.** A row like

```
  1 ◌ claude  found         --   no report yet
```

means the plugin located a running agent by scanning process environments, but that agent has
never fired a hook - normally because it was already running when the hooks were installed, or
because the plugin was reloaded. The row fills in the moment the agent next does anything. This
is not a broken install.

A discovered agent is found whether it was launched from a layout or typed into an existing
shell, so an empty panel with agents visibly running usually means the hooks are genuinely not
firing. Work through these in order:

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

## An agent in another Zellij session shows `found` but never live status

Agents in other sessions report status through a file in `$TMPDIR` rather than the pipe, which
reaches only their own session. A row stuck on `found` means that file is missing or not being
read. In order:

1. **Was the agent restarted since the spool shipped?** The hook is read at session start, so an
   agent started with an older hook writes nothing. This is the usual cause.
2. **Is anything being written?** Each agent gets one file:

   ```sh
   ls -la "${TMPDIR:-/tmp}/zj-agent-mob-$(id -u)/status/"
   ```

   One file per live agent, named `<session>.<pane_id>`. No files means the hook is not writing:
   check `ZJ_AGENT_SPOOL` is not set to `0`, and see
   [the panel says "no agents"](#the-panel-says-no-agents-in-this-session) for whether the hook
   runs at all.
3. **Is the record fresh?** Records older than 60s are ignored on read, so a paused agent falls
   back to `found`. `cat` one and check its `ts=` against `date +%s`.
4. **Did the pane id get recycled?** A record whose `session_id` disagrees with the running agent
   is ignored on purpose - it belongs to a previous agent on that pane. Restarting the agent
   rewrites it.

`found` is never wrong, only incomplete: the agent is really there and <kbd>Enter</kbd> still
jumps to it.

## Task text from other sessions is visible to other users

Only possible on a shared `/tmp`. The spool directory is created `0700` and namespaced by uid, so
another user cannot read it. If your `$TMPDIR` predates this and has looser permissions:

```sh
chmod 700 "${TMPDIR:-/tmp}/zj-agent-mob-$(id -u)"
```

Set `ZJ_AGENT_SPOOL=0` in the agent's environment to opt out of the spool entirely; status for
that agent's own session keeps working through the pipe.

## <kbd>x</kbd> does nothing on an agent from another session

Deliberate. Zellij's interrupt and close-pane calls act on the *current* session only, and pane ids
repeat across sessions - so signalling pane 3 from here would hit this session's pane 3, not the
one on the row. Press <kbd>Enter</kbd> to jump there first, then <kbd>x</kbd>.

## A row says `unknown` / `(session exited)`

Its Zellij session is no longer running, so nothing can report on it and its real state is
unknowable. The row is kept rather than dropped: an agent silently vanishing hides whether it
finished, crashed, or was never there.

<kbd>Enter</kbd> on such a row attaches (resurrects) the session rather than focusing a pane - the
pane no longer exists. <kbd>x</kbd> is refused: there is no process left to signal.

## The install screen says "Installer not found"

The plugin drives `~/.config/zj-agent-mob/install.sh`, which `init.sh` puts there. Nothing else creates it, so this means `init.sh` has never completed a run on this machine.

The usual cause is a partial install: the wasm was copied into `~/.config/zellij/plugins/` by hand (or by an earlier `install plugin` run), so the panel loads, but the hook and installer were never written. `init.sh status` reports `hook=absent` in that state.

Re-run the installer to bootstrap it:

```sh
curl -fsSL https://github.com/mohseenrm/zj-agent-mob/releases/download/v0.2.0/init.sh | sh
```

or `./init.sh` from a clone. Then press <kbd>r</kbd> on the install screen to re-read state.

Every install target shows `?` / "unknown" alongside this message: with no installer to run, the plugin genuinely cannot tell what is installed, so it reports nothing rather than guessing.

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

## Approve / reject from the panel does nothing

`a` and `r` only act when a prompt is actually parked for the selected agent, and the footer
shows `a approve  r reject` only then. If a prompt never appears:

1. **Is it enabled?** It is off by default. `ZJ_AGENT_APPROVE=1` must be set in the environment
   the agent itself runs in, not the panel's. Check with `echo $ZJ_AGENT_APPROVE` in the agent's pane.
2. **Did you restart the agent?** As with any hook change, it is read at session start.
3. **Is `PermissionRequest` registered?** Run `./init.sh status`, or check that the event is
   present and `async: false` in the settings file - an async hook cannot return a decision.
4. **Did it time out?** The hook waits `ZJ_AGENT_APPROVE_TIMEOUT` seconds (default 30) and then
   falls through to the agent's own in-pane prompt. That is the designed failure mode, not a bug.

The panel writes the verdict to `$TMPDIR/zj-agent-mob/verdict.<pane_id>`; watch that path to see
whether the keypress or the hook's read is the broken half.

## Hooks landed in my dotfiles repo

That's intended. `init.sh` resolves symlinks and writes through to the real file, so a stow-managed `~/.claude/settings.json` gets the change in your dotfiles repo, where you can commit it. The installer prints a note when it detects this.

## The panel is cramped or columns are missing

The layout degrades by width: the project column is dropped under 50 columns, and the per-agent detail line needs at least 60 columns plus two rows per agent. Resize the floating pane, or set a larger `width` / `height` in the layout.
