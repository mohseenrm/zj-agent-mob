Three fixes to how the panel presents itself, all of them visible the moment you open it on a wide screen.

## <kbd>U</kbd> is in the footer now

v0.13.0 made <kbd>U</kbd> work in every state, then only advertised it while an update was already pending - the one case where you don't need telling. If you were current, the key that checks for updates was nowhere on screen.

It is unconditional now, which is the honest thing: there is no state where pressing it does nothing.

```
↵ jump  g goto  / find  x kill  s sort  i install  U update  q hide
```

`d clear` gave up the slot. Dismissing a `done` badge also happens by visiting the pane, and <kbd>D</kbd> still clears the whole fleet at once.

## The panel fills the pane

`MAX_WIDTH` stops a task summary stretching across a huge monitor, which is right for text and wrong for everything else. On anything wider than 120 columns the rules and the footer stopped mid-air, leaving a ragged edge down the side of the float.

Text stays capped. The rules and the footer follow the pane.

## The running version is on screen

Right-aligned on the header row, so "which version am I actually on" no longer means opening the install screen:

```
zj-agent-mob   0 waiting · 1 working · 0 done                           v0.13.1
────────────────────────────────────────────────────────────────────────────────
```

It is dropped rather than truncated when the counts reach across to it - half a version number is worse than none.

## Upgrading

Press <kbd>U</kbd>. If you are on v0.13.0 the key already works, it was just hard to find; it is on the install screen (<kbd>i</kbd>) too.

```sh
curl -fsSL https://github.com/mohseenrm/zj-agent-mob/releases/download/v0.13.1/init.sh | sh
```

The hook script is unchanged from v0.12.1, so running agents don't need restarting. Panels in other sessions keep the old code until they reload.

**Full changelog:** https://github.com/mohseenrm/zj-agent-mob/compare/v0.13.0...v0.13.1
