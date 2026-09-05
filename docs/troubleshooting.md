# Troubleshooting

**Nothing showing up?** Start with [the panel says "no agents in this session"](#the-panel-says-no-agents-in-this-session).

- [Changes to the plugin seem to have no effect](#changes-to-the-plugin-seem-to-have-no-effect)
- [The plugin wasm in my dotfiles repo is out of date](#the-plugin-wasm-in-my-dotfiles-repo-is-out-of-date)
- [The panel says "no agents in this session"](#the-panel-says-no-agents-in-this-session)
- [An agent in another Zellij session shows `found` but never live status](#an-agent-in-another-zellij-session-shows-found-but-never-live-status)
- [Task text from other sessions is visible to other users](#task-text-from-other-sessions-is-visible-to-other-users)
- [<kbd>x</kbd> does nothing on an agent from another session](#x-does-nothing-on-an-agent-from-another-session)
- [<kbd>y</kbd> / <kbd>m</kbd> do nothing](#y--m-do-nothing)
- [No desktop notifications](#no-desktop-notifications)
- [A row says `gone` / `(session exited)`](#a-row-says-gone--session-exited)
- [A row in another session says `unknown`](#a-row-in-another-session-says-unknown)
- [The install screen says "Installer not found"](#the-install-screen-says-installer-not-found)
- [The install screen shows `?` / "unknown" for everything](#the-install-screen-shows---unknown-for-everything)
- [Zellij fails to load the plugin](#zellij-fails-to-load-the-plugin)
- [`waiting` stays on screen after you've answered](#waiting-stays-on-screen-after-youve-answered)
- [Approve / reject from the panel does nothing](#approve--reject-from-the-panel-does-nothing)
- [Hooks landed in my dotfiles repo](#hooks-landed-in-my-dotfiles-repo)
- [The panel is cramped or columns are missing](#the-panel-is-cramped-or-columns-are-missing)
- [The list says `↓ N more` and I cannot see every agent](#the-list-says--n-more-and-i-cannot-see-every-agent)
- [A row has a `!` next to it](#a-row-has-a--next-to-it)
- [Rows are grouped and I want the flat list back](#rows-are-grouped-and-i-want-the-flat-list-back)
- [A row says `wants: plan` / `wants: question` and <kbd>a</kbd> does nothing](#a-row-says-wants-plan--wants-question-and-a-does-nothing)

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

Agents in other sessions report status through a file in `$TMPDIR`, plus a direct pipe for the
states that need you. A row stuck on `found` means that file is missing or not being read. In
order:

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
3. **Is the record fresh?** A record older than 60s no longer refreshes a `working` row, which
   decays to `unknown`. `cat` one and check its `ts=` against `date +%s`. A blocked or finished
   agent is exempt: it writes nothing while it waits, so its unchanged record keeps re-confirming
   `waiting` / `done` for as long as the process is alive.
4. **Did the pane id get recycled?** A record whose `session_id` disagrees with the running agent
   is ignored on purpose - it belongs to a previous agent on that pane. The row follows the new
   agent as soon as that agent writes a record of its own, so quitting an agent and starting
   another in the same pane recovers on the next event rather than leaving the dead agent's last
   status on screen.

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

Foreign rows are killed through the `zellij` binary rather than the plugin's own calls, which act
on the current session only. So this means one of:

1. **The row's session has exited.** <kbd>x</kbd> is refused when there is no process left to
   signal; the row reads `unknown` / `(session exited)`.
2. **`zellij` is not on the panel's `PATH`.** The plugin shells out to it for foreign rows. It is
   the same binary the hook needs, so this usually shows up as nothing reporting at all.

A cross-session action that fails prints the reason under the list (`kill failed: ...`) rather
than silently removing the row, so check there first. Jumping with <kbd>Enter</kbd> and pressing
<kbd>x</kbd> locally always works.

## <kbd>y</kbd> / <kbd>m</kbd> do nothing

They only act while the selected agent is actually blocked (`waiting` or `idle-wait`), and the
footer shows `y yes  m message` only then. Typing into an agent mid-turn would land as stray input
in its prompt, so the keys are inert for every other status.

## No desktop notifications

In order:

1. **Is a notifier installed?** The plugin probes for `terminal-notifier`, then `osascript`, then
   `notify-send`. On macOS `osascript` is always present; on Linux install `notify-send`
   (`libnotify-bin`). With none of them, notifications are silently disabled.
2. **Is the panel already on screen?** Notifications are suppressed while it is visible, since it
   is already telling you.
3. **Did the same agent just notify?** Each agent is limited to one notification per
   `notify_cooldown` seconds (default 60), so a flapping row cannot spam you.
4. **Is the status one you asked about?** The default is `waiting,failed`. `done` is opt-in via
   `notify "waiting,failed,done"`.
5. **Are notifications allowed at the OS level?** On macOS, check System Settings > Notifications
   for your terminal. Verify the exact call the plugin makes:

   ```sh
   osascript -e 'on run argv
   display notification (item 2 of argv) with title (item 1 of argv)
   end run' 'zj-agent-mob' 'test'
   ```

## A row says `gone` / `(session exited)`

Its Zellij session is no longer running, so nothing can report on it and its real state is
unknowable. The row is kept rather than dropped: an agent silently vanishing hides whether it
finished, crashed, or was never there.

<kbd>Enter</kbd> on such a row attaches (resurrects) the session rather than focusing a pane - the
pane no longer exists. <kbd>x</kbd> is refused: there is no process left to signal.

## A row in another session says `unknown`

The process scan can see the agent, so it is running, but no status has reached the panel in the
last 60 seconds. This is distinct from `gone`, where the whole session has exited.

The panel re-reads the spool every 5 seconds, so a poll that is merely late is ruled out. Look at
the agent's record - the filename is `<session>.<pane_id>`:

```sh
cat "${TMPDIR:-/tmp}/zj-agent-mob-$(id -u)/status/"*
```

**No file for that agent.** The hook never ran, so the agent is not reporting at all. Re-run
`init.sh`, then **restart that agent**: hooks are read at startup, so an already-running agent
keeps the old configuration.

**A file with `status=` empty**, like:

```
ts=1787407612,pane_id=2,session=dotfiles-new,tool=claude,status=,session_id=21cb996b-...
```

This is a hook older than v0.5.1. Counter events (`SubagentStart`, `TaskCreated`) carry no status,
and that hook wrote the empty value into the record anyway. The panel cannot parse a statusless
record, so it skips it - and since nothing overwrites that file afterwards, every later poll skips
it too and the row never recovers. An agent that spawns a subagent gets stuck this way.

Fix it by updating the hook and clearing the record:

```sh
./scripts/reinstall-local.sh   # or re-run init.sh from a v0.5.1+ release
```

Then restart that agent. Newer hooks inherit the previous status instead of blanking it, and skip
the write entirely when there is nothing to inherit.

Two benign cases also read `unknown`, both self-correcting:

- `ZJ_AGENT_SPOOL=0` is set for that agent, which opts it out of the cross-session transport. Its
  status still reaches a panel in its own session.
- The agent is genuinely mid-turn and quiet. Only `working` and `compact` decay this way - a
  blocked or finished agent keeps its status, since silence is what those states predict.

## The install screen says "Installer not found"

The plugin drives `~/.config/zj-agent-mob/install.sh`, which `init.sh` puts there. Nothing else creates it, so this means `init.sh` has never completed a run on this machine.

The usual cause is a partial install: the wasm was copied into `~/.config/zellij/plugins/` by hand (or by an earlier `install plugin` run), so the panel loads, but the hook and installer were never written. `init.sh status` reports `hook=absent` in that state.

Re-run the installer to bootstrap it:

```sh
curl -fsSL https://github.com/mohseenrm/zj-agent-mob/releases/download/v0.10.0/init.sh | sh
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

1. **Is it switched off?** It is on by default, so check nothing set `ZJ_AGENT_APPROVE=0` in the
   environment the agent itself runs in, not the panel's. Check with `echo $ZJ_AGENT_APPROVE` in
   the agent's pane. An agent started before the hooks were installed also has to be restarted.
2. **Did you restart the agent?** As with any hook change, it is read at session start.
3. **Is `PermissionRequest` registered?** Run `./init.sh status`, or check that the event is
   present and `async: false` in the settings file - an async hook cannot return a decision.
4. **Did it time out?** The hook waits `ZJ_AGENT_APPROVE_TIMEOUT` seconds (default 30) and then
   falls through to the agent's own in-pane prompt. That is the designed failure mode, not a bug.

   The panel stops offering <kbd>a</kbd> / <kbd>r</kbd> at the same moment: the prompt box and the
   `a approve  r reject` hints disappear once the hook has stopped reading, so a keypress can no
   longer report an approval that would reach nobody. If the box vanished while you were reading
   it, answer in the pane itself - <kbd>Enter</kbd> jumps there. Raise `ZJ_AGENT_APPROVE_TIMEOUT`
   if you want longer to decide.

The panel writes the verdict to `$TMPDIR/zj-agent-mob/verdict.<pane_id>`; watch that path to see
whether the keypress or the hook's read is the broken half.

## Hooks landed in my dotfiles repo

That's intended. `init.sh` resolves symlinks and writes through to the real file, so a stow-managed `~/.claude/settings.json` gets the change in your dotfiles repo, where you can commit it. The installer prints a note when it detects this.

## The panel is cramped or columns are missing

The layout degrades by width: the project column is dropped under 50 columns, and the per-agent detail line needs at least 60 columns plus two rows per agent. Resize the floating pane, or set a larger `width` / `height` in the layout.

## The list says `↓ N more` and I cannot see every agent

Working as intended. The panel clips the list to the pane rather than printing
rows past the bottom edge, which used to take the footer rule and the key hints
with them.

The viewport follows the selection, so <kbd>j</kbd> / <kbd>k</kbd> past the last
visible row scrolls rather than stopping. <kbd>1</kbd>–<kbd>9</kbd> still reach
the first nine rows wherever the view is, and <kbd>g</kbd> opens a count for any
row by its printed number - `g25`<kbd>Enter</kbd>, or `g25G` if you think in vim.
<kbd>gg</kbd> and <kbd>G</kbd> are the first and last rows.

To see more at once, make the floating pane taller (`height` in the layout).
Under about two rows per agent the per-agent detail line is dropped first, which
roughly doubles how many rows fit.

## A row has a `!` next to it

That agent fired a desktop notification since you last had the panel focused. It
exists so coming back from a banner does not mean re-scanning the whole list for
whichever row changed.

Every marker clears the moment the panel becomes visible, so if they persist the
plugin is not receiving `Visible` events - which would also mean notifications
are firing while the panel is on screen. Nothing to do about it from the panel;
it points at the Zellij session rather than the plugin.

No markers ever appearing is the normal state when notifications are off. See
[no desktop notifications](#no-desktop-notifications).

## Rows are grouped and I want the flat list back

<kbd>s</kbd> cycles the ordering: urgency (the default) -> grouped by project ->
grouped by session -> back to urgency. Press it until the header chip disappears;
the chip is only shown while grouping is on, so no chip means the flat
urgency-ordered list.

Grouping never reorders by name alone: each group takes the rank of its most
urgent member, so a blocked agent stays at the top even if its project sorts
last alphabetically. If a group looks out of order, it is being ranked by a row
you may have to scroll to see.

Group headings cost one row each, which is one fewer row for agents. On a short
pane that can be the difference that drops the per-agent detail line - see
[the panel is cramped](#the-panel-is-cramped-or-columns-are-missing).

## A row says `wants: plan` / `wants: question` and <kbd>a</kbd> does nothing

Correct, and the label is telling you why. Only `wants: permission` is a yes/no
the panel can answer:

| `wants:` | <kbd>a</kbd> / <kbd>r</kbd> | What to do instead |
|---|---|---|
| `permission` | Works by default; `ZJ_AGENT_APPROVE=0` opts out | - |
| `plan` | No | <kbd>Enter</kbd> to the pane and read it |
| `question` | No | <kbd>m</kbd> to type a reply, or jump to the pane |
| `idle` | No | Nothing is blocked on a decision |

A `waiting` row with no `wants:` at all came from a hook that predates the field,
or from a spool record written by an older installed hook. Restart the agent
after upgrading - hooks are read at session start.
