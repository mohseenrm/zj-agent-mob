The panel now updates itself. When a newer release is out, a line shows up in the footer:

```
update available: v0.12.0 (press U)
```

Press <kbd>U</kbd> and the panel downloads the release, swaps the wasm, hook script and installer into place, and reloads itself. No shell, no reinstall, no leaving Zellij. This should be the last release you install by hand.

## How it works

On load the panel asks the installed `install.sh` for the latest tag:

```sh
~/.config/zj-agent-mob/install.sh check-update
```

The answer is cached for six hours in `~/.config/zj-agent-mob/update-check`, so ten sessions starting at once make one request. <kbd>U</kbd> then runs `install.sh --version <tag> plugin` and reloads the plugin once it exits cleanly. A failed update leaves the running version alone and prints the first error line in the footer; <kbd>U</kbd> retries.

Some details worth knowing:

- Updates never touch your agent hook settings. If you deliberately hooked only Claude or only Codex, it stays that way.
- Other Zellij sessions keep the old code until they reload. Their next <kbd>U</kbd> finds the files already current and just reloads.
- The install screen (<kbd>i</kbd>) now shows the running version in its header.
- Rather not have the panel phone GitHub? Turn it off in the plugin config:

```kdl
LaunchOrFocusPlugin "file:~/.config/zellij/plugins/zj-agent-mob.wasm" {
    floating true
    check_updates false
}
```

## Installer fixes

Two `init.sh` bugs the update path would have tripped over, fixed for manual installs too:

- A from-release run used to copy its stale self over `~/.config/zj-agent-mob/install.sh` forever; it now fetches the installer from the release like everything else.
- The wasm is swapped in with a rename instead of a plain `cp`, so a reload can never catch it half-written and two sessions updating at once cannot race.

## Upgrading

One last time by hand:

```sh
curl -fsSL https://github.com/mohseenrm/zj-agent-mob/releases/download/v0.12.0/init.sh | sh
```

Then reload the plugin, since Zellij caches compiled builds:

```sh
zellij action launch-or-focus-plugin --skip-plugin-cache --floating \
  "file:$HOME/.config/zellij/plugins/zj-agent-mob.wasm"
```

The hook script is unchanged from v0.11.x, so running agents don't need restarting.

**Full changelog:** https://github.com/mohseenrm/zj-agent-mob/compare/v0.11.1...v0.12.0
