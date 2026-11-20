<p align="center">
  <img src="assets/worminal.svg" alt="Worminal logo" width="88">
</p>

# Worminal

Your shells, web tools and remote machines, together in a small Linux terminal.

[Visit the website](https://flintwinters.github.io/worminal/).

Worminal keeps your workspace alive independently of its windows. Use it yourself, let an agent work alongside you,
and come back without starting over.

- **Work that stays running.** Close every window and return to the same tabs, shells and running programs.
  Local and remote sessions live in a background service.

- **Multiple machines, one interface.** Keep local and SSH-backed tabs in the same window. Work with headless
  hosts, enroll devices in a shared registry, and check their availability when you need them.
  [Remote connections](docs/remote-install.md) · [Devices](docs/devices.md)

- **Web tools belong beside your shell.** A shell command can turn its tab into a web page or local HTML tool.
  Build a file explorer, report or project dashboard as an ordinary web document. Local tools can use a limited,
  read-only file and directory API; edit and reload them without rebuilding the terminal.

- **Agents can work with your actual sessions.** A discoverable native API lets agents find tabs, read screens
  and retained output, wait for changes, and send guarded input locally or over SSH. Inspection leaves your focus,
  scroll position and terminal size alone. Optional shell integration adds command completion and exit status.
  [Agent interface](docs/automation.md)

- **Updates keep your shells alive.** Daemons transfer live sessions and windows replace themselves automatically.
  Browser pages reload with their profile and cookies retained. Failed handoffs keep the existing owner running.
  [Live updates](docs/window-updates.md)

- **Arrange work as you go.** Drag tabs onto pane edges to split a window, move them between panes, and cycle
  through them with the keyboard or Ctrl+scroll. Shells and browser tools share the same workspace.

- **See what needs attention.** Tab colors distinguish busy tasks, SSH sessions and background shell jobs.
  Applications can send desktop notifications from local or remote tabs; clicking one takes you to its tab.
  [Notifications](docs/notifications.md)

- **Bring your settings across machines.** Reuse supported font, color and scrollback settings from an Alacritty
  config. Worminal settings sync when connecting to peers, merging independent edits and reporting conflicts.
  Compatible peers also sync daemon versions automatically. [Configuration](docs/config.md)

- **Keep the terminal small.** Native Wayland and X11 support, a lightweight launcher, and a shared terminal engine.
  Hidden busy tabs avoid streaming terminal frames. CPU and network limits are checked as part of the test suite.

Built with Rust, GTK and WebKitGTK, with a terminal engine derived from [st](https://git.suckless.org/st/).
[License](LICENSE).
