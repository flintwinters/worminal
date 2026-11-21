<p align="center">
  <img src="assets/worminal.svg" alt="Worminal logo" width="88">
</p>

# Worminal

All your remote machines together in one terminal.

Worminal keeps your workspace alive independently of its windows. Use it yourself, let an agent work alongside you,
and come back without starting over.

- **Keep local and remote tabs in the same window.** Work with headless
  hosts, enroll devices in a shared registry, and check their availability when you need them.
  [Remote connections](docs/remote-install.md) · [Devices](docs/devices.md)

- **Use Alacritty configs.** Reuse settings from an Alacritty config. Worminal settings sync when connecting to peers, merging independent edits and reporting conflicts.
  Compatible peers also sync daemon versions automatically. [Configuration](docs/config.md)

- **Keep the terminal small.** Native Wayland and X11, with Windows support through WSL.

- **Built-in browser** worminal is also a WebKit based browser forked from [surf](https://surf.suckless.org/).  Which means you can *actually* browse the web in your terminal.

- **Agents can work with your actual sessions.** A highly discoverable API lets agents find tabs, read screens
  and retained output, wait for changes, and send guarded input locally or over SSH.
  [Agent interface](docs/automation.md)
  	- Know when your agents are working or get notified when they're stuck

- **Split screen.** Control view splitting with both mouse and keyboard. Windows reopen with their splits, dividers,
  size and position after an accidental close or a crash. [Layout restore](docs/layout-restore.md)

- **Bookmarks.** Star a tab (or press Ctrl+Shift+D) to save a terminal's directory and host, or a web page, then
  reopen it from a keyboard-driven side panel (Ctrl+Shift+B). [Bookmarks](docs/bookmarks.md)

Built with Rust, GTK and WebKitGTK, with a terminal engine derived from [st](https://st.suckless.org/).
[License](LICENSE).

[Visit the github pages site](https://flintwinters.github.io/worminal/).
