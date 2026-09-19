<kbd>U</kbd> updates the plugin. One press: it asks GitHub for the latest release, downloads it, installs it, and reloads the panel in place. No clone, no re-running `init.sh`, no restart.

The key existed in v0.12.0 but only did something in a narrow window. It installed the release a background check had already found, and the background check runs once per panel load off a six-hour cache. Press <kbd>U</kbd> any other time - a panel opened this morning, a release that landed an hour ago, `check_updates false` - and nothing happened. No error, no feedback. A key that silently does nothing is a key you stop trusting.

## What changed

<kbd>U</kbd> now does its own checking when it has to.

- **Nothing known?** It runs the check itself with `--force`, going past the cache to ask GitHub directly, then installs whatever comes back.
- **Release already known?** Straight to installing, no second round trip.
- **Already current?** It says so, rather than leaving you staring at an unchanged screen.

```
checking for updates...
update available: v0.14.0 (press U)
updating...
```

or

```
already on the latest release (v0.13.0)
```

The background check is unchanged and still optional. It is a nudge now, not a prerequisite - `check_updates "false"` stops the automatic check, and <kbd>U</kbd> keeps working.

## The key is on screen now

The list footer is at its 84-column budget, so `U update` could not simply be added to it. While an update is waiting, `g goto` and `d clear` step aside for it:

```
↵ jump   / find   x kill   s sort   i install   U update   q hide
```

Both keys still work, and neither is what you came to the panel for with a release sitting there unread. The install screen (<kbd>i</kbd>) advertises <kbd>U</kbd> unconditionally, since there it always acts.

## Under the hood

`init.sh check-update` takes a `--force` flag that skips the cache:

```sh
./init.sh check-update          # cached, six hours
./init.sh check-update --force  # asks GitHub now
```

The two checks stay separate on purpose. The background one may report but never installs - it runs without anyone asking, and acting on it would update the panel out from under whoever is using it. Only the hand-fired check chains into an install.

A tag from the network is interpolated into a shell command, so an unparseable one is dropped rather than passed along. That was true before and still is.

## Upgrading

From v0.12.x, press <kbd>U</kbd> - the old narrow path is enough to get you here if the footer is showing an offer. Otherwise:

```sh
curl -fsSL https://github.com/mohseenrm/zj-agent-mob/releases/download/v0.13.0/init.sh | sh
```

The hook script is unchanged from v0.12.1, so running agents don't need restarting. Panels in other sessions keep the old code until they reload.

**Full changelog:** https://github.com/mohseenrm/zj-agent-mob/compare/v0.12.1...v0.13.0
