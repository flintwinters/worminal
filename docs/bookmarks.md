# Bookmarks and the side panel

Bookmarks reopen a terminal tab or a web page without retyping its path. A terminal bookmark saves its working
directory and, when its daemon is on another host, that SSH host. It never saves a command: opening one starts a
fresh shell, exactly as restart recovery does. A web bookmark saves its URL and title.

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
```

Each entry has exactly one of `directory` (an absolute path) or `url`. `master` belongs to a `directory` and is an
SSH host or `user@host`. Other keys, duplicates, a relative directory, and more than 256 entries or 64 KiB are
rejected. Order is kept; the app adds new bookmarks at the end.

Windows re-read the file when they gain focus, so a change made in another window or by hand shows up without any
file watching or polling. Changes are written under a lock and replaced atomically; if another window holds the
lock the action reports that and can simply be repeated. A damaged or symlinked file is reported and left exactly
as it is: it is never reset or overwritten, so fix or remove it by hand. Comments in the file are lost when the
app rewrites it.
