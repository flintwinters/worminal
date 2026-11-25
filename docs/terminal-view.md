# Terminal-only view

On a Linux host with Worminal installed, run:

```sh
worminald terminal
```

From Windows Terminal or another terminal with an SSH client, run:

```sh
ssh -t USER@HOST worminald terminal
```

Use the Linux host that owns the workspace as `HOST`. The SSH client allocates a terminal on that host, where
Worminal draws the entire view, including the tab bar. The headless Worminal install includes this command.
On any client operating system, all you need is SSH and a VT-compatible terminal.

The view starts the daemon if needed and displays an ordered tab bar above the first terminal pane.
Terminal tabs use the same directory or SSH-user names as the graphical tab bar.
The tab bar shares the desktop view’s character-cell layout, palette, and bookmark stars.
Tabs wrap in order, with the active tab visible when the display can only fit part of the bar. On startup it opens a shell if the
workspace has no terminal tabs. It borrows the active tab's viewport and releases it when switching or detaching.
The daemon keeps the shells and their scrollback alive between connections.

| Type Ctrl+] then | Action |
| --- | --- |
| `n` or `p` | Select the next or previous terminal tab |
| `t` | Open a shell |
| `k` or `j` | Scroll up or down |
| `q` | Detach |
| Ctrl+] | Send a literal Ctrl+] to the shell |

The view displays one pane and skips browser tabs. It forwards VT keyboard sequences and mirrors application
cursor, keypad, and bracketed paste modes. The outer terminal supplies the font.

Click a tab to select it, drag it onto another tab to reorder it, or middle-click it to close its shell.
Scroll over the bar or use Ctrl+wheel to cycle. Ctrl+T and Ctrl+W open and close tabs when forwarded by the client.
Click a visible star or use Ctrl+Shift+D to toggle a directory bookmark.
Mouse reporting uses SGR mouse mode. In terminals that reserve Shift+drag for local selection, use that gesture
to select text in the outer terminal.

Ctrl+Tab and Ctrl+Shift+Tab cycle when the client forwards them. Use the tab bar or Ctrl+] then n/p
when the client handles those keys itself.
