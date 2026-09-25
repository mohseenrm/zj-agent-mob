First stable release. Two new things: you can pin rows, and the status icons are Nerd Font glyphs.

![Pinning two agents to the top of the panel, and the new Nerd Font status icons](https://raw.githubusercontent.com/mohseenrm/zj-agent-mob/v1.0.0/demo/v1-pinning-and-icons.gif)

## Pinning

Press <kbd>p</kbd> to pin the selected row. Pinned rows sit in a block at the top, above everything else, whatever their status.

```text
zj-agent-mob   1 failed · 1 waiting · 2 working · 1 done                 v1.0.0
────────────────────────────────────────────────────────────────────────────────
  pinned (2)
  1  claude  working     4m12s  zj-agent-mob/feat-pins    refactor sort_agents
  2  claude  waiting     0m40s  other                     approve Bash?
  api (1)
  3  codex   failed     12m01s  api                       rate limit
  zj-agent-mob (1)
  4  claude  done        8m10s  zj-agent-mob              stow tree
```

The list sorts by urgency. That's right for "who needs me" and wrong when you're watching two specific agents, because their row number moved every time anything else changed status. Now it doesn't, so <kbd>1</kbd>–<kbd>9</kbd> and <kbd>g</kbd> are stable addresses for the rows you picked.

Inside the block, urgency still decides the order. Pinning changes which rows are on top, not how they rank against each other.

**Grouping.** The block gets its own heading in every mode, including urgency, which otherwise has none. Without it, a pinned `working` row above an unpinned `failed` one just looks like a sorting bug.

A pinned row leaves its project or session group, and that group's count drops it: `api (1)`, not `(2)`. Group ranking works the same way, so pinning a group's only blocked agent lets the group fall back down the list.

**Persistence.** Pins live in a `pins` file next to the status records. Pin something in one session and every panel shows it pinned, including after a reload. Pins follow the agent, not the row, so they're dropped when the row goes away and a recycled pane ID never inherits one.

<kbd>/</kbd> ignores pins, so search is still the fastest way to see everything in pure rank order.

## Icons

| State | Before | After |
|---|:---:|:---:|
| waiting | `●` |  |
| idle-wait | `◐` |  |
| failed | `✗` |  |
| done | `✓` |  |
| idle | `○` |  |
| discovered | `◌` |  |
| session gone | `?` |  |
| compact | *shared the spinner* |  |
| subagent fan-out | `⑂` |  |

`done` and `failed` are now the same shape in different colours, which helps when scanning a column. `compact` has its own icon instead of borrowing the spinner, and a row that notified you carries a small dot in the gutter.

Two of the old glyphs were wrong for the job: the fan-out badge was an OCR character that most fonts render as a blob, and a dead session was an ASCII `?`, the only letter in a column of symbols.

### Without a Nerd Font

The glyphs render as boxes. Either [install a patched font](https://www.nerdfonts.com):

```sh
brew install --cask font-jetbrains-mono-nerd-font
```

Or switch back to the old symbols:

```kdl
LaunchOrFocusPlugin "file:~/.config/zellij/plugins/zj-agent-mob.wasm" {
    floating true
    icons "unicode"
}
```

Columns line up either way. Every glyph in both sets is exactly one cell wide, and the build checks it.

## Footer

`p pin` took the slot `q hide` had. <kbd>q</kbd> and <kbd>Esc</kbd> both still hide the panel.

```text
↵ jump  g goto  / find  p pin  x kill  s sort  i install  U update
```

## On 1.0

These are stable now, and breaking any of them means a major version:

- key bindings
- the `zellij pipe --args` protocol the hooks speak
- spool records and panel beacons
- KDL config keys

## Upgrading

Press <kbd>U</kbd>, or:

```sh
curl -fsSL https://github.com/mohseenrm/zj-agent-mob/releases/download/v1.0.0/init.sh | sh
```

The hook script hasn't changed since v0.12.1, so running agents don't need a restart. Panels in other sessions keep the old code until they reload.

**Full changelog:** https://github.com/mohseenrm/zj-agent-mob/compare/v0.13.1...v1.0.0
