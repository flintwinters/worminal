<p align="center">
  <img src="assets/worminal.svg" alt="Worminal logo" width="88">
</p>

# Worminal

Worminal is a small Linux terminal that keeps your work independent of its windows: shells and tabs live in a background service, locally or over SSH, so closing every window leaves your programs running.

Its integrated browser lets a shell command turn the current terminal tab into a web page or local HTML tool, putting graphical tools directly into your shell workflow. Local HTML tools can read files and list directories through a limited, read-only API, so something like a file explorer can live beside your shells as an ordinary web document you can edit and reload without rebuilding Worminal.

Updates [replace open windows automatically](docs/window-updates.md) while keeping terminal processes alive.
Browser pages reload with their profile and cookies retained.

The aim is to make the terminal useful for more of your work while keeping the interface small and memory use low.

Worp is Worminal's high-level protocol for abstracting away the communication medium. It is an API layer that allows
Worminal nodes to not care about the underlying method used to send messages: TCP, UDP, SSH, HTTP, etc.
See [Worp architecture](docs/worp.md) for its shared command/event contract and currently supported transports.
Native windows and other clients use the same Worp operations, including catalog/frame subscriptions, input and tab lifecycle.

Agents can discover and control existing terminal sessions with `worminald api help`;
see [the agent interface](docs/automation.md) for listing tabs, reading screens and sending keystrokes.
Shell applications can send [OSC 99 desktop notifications](docs/notifications.md), including from remote tabs.
A small [explicit device registry](docs/devices.md) shares enrolled SSH routes through one authority, with
on-demand availability checks and no heartbeats.
Use [`worminald install-on-remote HOST`](docs/remote-install.md) to bootstrap a new compatible headless node over SSH.
For existing devices, [`python3 manage.py upgrade HOST`](docs/remote-upgrade.md) copies this device’s installed release.

Installations include Atkinson Hyperlegible Mono as the default font, at 9 pt with a -3 pixel line offset.
Explicit Alacritty font settings at build time and the `-f` command-line option override the defaults.

Built with Rust, GTK, WebKitGTK, and a terminal engine derived from [st](https://git.suckless.org/st/); see [LICENSE](LICENSE).
