# Bookmarks and the side panel

Bookmarks reopen a terminal tab or a web page without retyping its path. A terminal bookmark saves its working
directory and, when its daemon is on another host, that SSH host. It never saves a command: opening one starts a
fresh shell, exactly as restart recovery does. A web bookmark saves its URL and title. A server bookmark saves
only an SSH host or `user@host`, for quick access to a machine.

## Making and opening bookmarks

- A tab with a known directory (or a web page with a URL) shows a star before its name. Click the star, or press
  **Ctrl+Shift+D**, to bookmark the tab; do it again to remove the bookmark. A filled star is a bookmark.
  Tabs that share a directory share its bookmark. A tab with an `ssh` client in its foreground has no star: the
  directory Worminal knows for it is the local shell's, not the remote one, so it cannot be bookmarked. Bookmark
  the remote place from a tab whose daemon runs there (`--master`).
- **Ctrl+Shift+B** (or **Ctrl+Shift+P**, for keyboards that can send it) opens the side panel beside the panes. Pressed while the panel is open but the keyboard is
  elsewhere, it moves the keyboard to the panel; pressed with the keyboard already there, it closes the panel.
- In the panel: **Up/Down** (or `k`/`j`) move, **Enter** (or a click) opens the bookmark in a new tab and closes the
  panel, **Delete** removes it, **Left/Right** (or `h`/`l`, Tab) switch panel tabs, **Escape** closes the panel. The
  new tab takes the keyboard. Home/End and Page Up/Down jump.
- **a** asks for a server. Type `host` or `user@host`, with `:port` for a port other than 22 (`user@host:2222`, `[::1]:2222`), and press Enter, then give it a name (optional: a blank
  answer keeps none; names may contain spaces) and press Enter again to save it. Escape cancels, Ctrl+Shift+V
  pastes. The panel's own letters (`a`, `j`, `x`, `q`...) are ordinary text while you type, and an entry that
  cannot be an SSH endpoint is refused with the reason, nothing saved. Press Ctrl+U to clear what you typed.
- **r** names the selected bookmark of any kind, starting from its current name; a blank answer removes the name.
  A named bookmark is listed by its name; an unnamed one by its path, page address or `ssh user@host`.
- A server bookmark opens a new tab on this machine that runs `ssh` to it. When ssh
  ends, whether by logout, a refused connection or Ctrl+C, the tab stays open in your shell with ssh's message
  above, instead of closing. The server needs nothing installed. A server bookmark cannot open in a window that
  shows another host, since the ssh would start from that host.
- A terminal bookmark opens on the daemon that owns it: this window's own, a host it already shows, or a new
  connection to that host, as `--master` adds one. A local bookmark cannot open in a window that shows another host.

The panel is built when opened and freed when closed, so a window that never opens it pays nothing. Its list is
the first panel tab, Bookmarks; other lists (devices, open tabs, recently closed tabs) can be added as further
tabs without changing the panel.

## The file

Bookmarks live in `$XDG_CONFIG_HOME/worminal/bookmarks.yaml` (default `~/.config/worminal/bookmarks.yaml`). It
belongs to you, like `devices.yaml`: it is hand-editable, shared by every window and workspace scope, and never
layered or synced between devices.

```yaml
bookmarks:
  - directory: /home/me/project     # a terminal
    title: project                  # optional
  - directory: /srv/app
    master: me@build-host           # the SSH endpoint whose daemon owns that directory
  - url: https://example.org        # a web page
    title: Example
  - host: me@build-host              # a server: opens a tab that runs ssh to it
    port: 2222                      # optional; without it ssh uses its default (22, or what ~/.ssh/config says)
    title: Build box                # any entry may have a name; the panel lists it instead of the address
```

A `port` (1-65535) belongs to a `host` and opens as `ssh -p PORT host`; the same host on another port is another
bookmark. Port 22 is the default, so typing `:22` (or writing `port: 22`) is the same as no port. A typed `host:abc`
or `host:99999` is refused instead of becoming a host name.

Each entry has exactly one of `directory` (an absolute path), `url` or `host`. `master` belongs to a `directory`;
both it and `host` are an SSH host or `user@host`, checked like a device endpoint (no leading `-`, no spaces or
shell characters, so an entry can never become an ssh option or a command). Other keys, duplicates, a relative directory, and more than 256 entries or 64 KiB are
rejected. Order is kept; the app adds new bookmarks at the end.

Windows re-read the file when they gain focus, so a change made in another window or by hand shows up without any
file watching or polling. Changes are written under a lock and replaced atomically; if another window holds the
lock the action reports that and can simply be repeated. A damaged or symlinked file is reported and left exactly
as it is: it is never reset or overwritten, so fix or remove it by hand. Comments in the file are lost when the
app rewrites it.
