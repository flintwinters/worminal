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
The active tab is highlighted and stays visible when the terminal is narrow. On startup it opens a shell if the
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
cursor, keypad, and bracketed paste modes. The outer terminal supplies the font and default palette.
