<p align="center">
  <img src="assets/worminal.svg" alt="" width="88">
</p>

# Worminal

**Close your windows. Keep your shells.**

Worminal is a small Linux terminal with shared tabs and a built-in browser, running on Wayland and X11.
Your workspace belongs to a background service rather than a window: leave commands running, close every
window, and pick up where you left off. Keep that service on a remote machine to use the same workspace over SSH.

## Why use it?

- **Windows are disposable; your work stays put.** Open another window onto the same ordered tabs.
  Closing a window leaves shells running. Closing a tab ends it everywhere.
- **A remote workspace with a local interface.** `worminal --master user@host` connects to shells on that
  machine, with native local windows, clipboard access, and scrollback. No remote desktop required.
- **Web pages beside your shells.** WebKitGTK tabs put documentation, local HTML tools, and a
  [file explorer](file-explorer/README.md) in the same workspace. Browser support loads on first use.
- **A small interface with familiar terminal behavior.** Directory names label terminal tabs;
  bounded scrollback reflows when you resize. Copy and paste, clickable links, and font zoom are built in.
- **Bring your Alacritty appearance.** Reuse its colors, font, spacing, and history settings at build time.
  Worminal favors low memory use and responsive input over a large interface or settings system.

Worminal uses Rust for the application and workspace service, a terminal engine derived from
[st 0.9.3](https://git.suckless.org/st/), and GTK 3/Pango-Cairo for rendering.

## Get started

Building requires Python 3.11+, a current stable Rust toolchain with Cargo, a C compiler, `pkg-config`,
and GTK 3, WebKitGTK 4.1, and libclang development libraries. On Debian/Ubuntu, install the native dependencies:

```sh
sudo apt install build-essential pkg-config python3-venv libclang-dev libgtk-3-dev libwebkit2gtk-4.1-dev
```

With Rust and Cargo installed, build from the repository root:

```sh
python3 -m venv build/venv
. build/venv/bin/activate
python3 -m pip install -r tools/requirements.txt
python3 manage.py build
./build/bin/worminal
```

To install system-wide, build as your regular user so the build reads your theme, then install that build:

```sh
sudo -n build/venv/bin/python manage.py install --prefix /usr/local --skip-build
```

Python and `manage.py` are development tools. Launch the installed application with `worminal`.
The native build does not require npm or the optional file-explorer submodule.

## Your workspace

```sh
worminal                         # Open a window
worminal --attach                # Return to existing tabs without adding a shell
worminal --master user@host       # Open a workspace on another machine
worminal --surf https://example.org
```

For a remote workspace, install Worminal on both machines and put `worminald` on the remote account's `PATH`.
Windows connected to the same service share tabs and select them independently. Each terminal tab has one
live view at a time; other windows showing it catch up when focused or used.

Shells survive closed windows while the service keeps running. After a service or computer restart,
Worminal restores tab order, working directories, and browser URLs using fresh shells.
Running programs and scrollback do not survive a restart. Browser pages run locally;
remote workspaces share their saved URLs, not their live page state.

From inside a Worminal terminal, turn its tab into a browser:

```sh
worminal --new-browser-tab https://example.org
worminal --new-browser-tab /absolute/path/to/page.html
```

**This replaces the calling terminal and ends its shell.** Use `--surf` to open a browser in a new window instead.
Bare hosts default to HTTPS; existing paths open as local files.
See the [file explorer instructions](file-explorer/README.md) for the optional offline directory browser.

## Everyday controls

| Action | Shortcut |
| --- | --- |
| New terminal tab | Ctrl+Shift+T |
| New window | Ctrl+Shift+N |
| Close tab everywhere | Ctrl+Shift+W |
| Next / previous tab | Ctrl+Tab / Ctrl+Shift+Tab |
| Select tab 1–8 / last tab | Alt+1–8 / Alt+9 |
| Copy / paste | Ctrl+Shift+C / Ctrl+Shift+V |
| Smaller / larger font | Ctrl+- / Ctrl+= |
| Scroll history | Mouse wheel / Shift+PageUp / Shift+PageDown |
| Browser address / find / reload | Ctrl+K / Ctrl+F / Ctrl+R |

Plain Ctrl+N reaches terminal applications. Hold Shift while dragging to select text in applications
that capture the mouse. Links open in your local browser, including links printed by remote shells.

## Appearance

The build reads your Alacritty TOML or YAML configuration from the XDG config directory
(normally `~/.config/alacritty`), including imports. To choose a file explicitly:

```sh
WORMINAL_ALACRITTY_CONFIG=/path/to/alacritty.toml python3 manage.py build
```

Settings are compiled in: rebuild and reinstall after changing them, then reopen all windows to load
the new frontend. There is no configuration file to parse at launch.

## Further reading and development

- [Command reference](docs/man/worminal.1): launch options, restart recovery, service shutdown, and session transfer.
- [Window-state API](docs/window-state.md): working-directory and focus integration over session D-Bus.
- [Architecture and migration](docs/rust-migration.md): service ownership, upgrade recovery,
  and measured resource costs.
- [Agent guidance](AGENTS.md): project constraints, architecture, and current investigations.

`python3 manage.py --help` lists internal build, test, and measurement commands.
`python3 manage.py check` runs the regression suite on private virtual displays;
`latency-visible`, `interaction-perf`, and `browser-perf` measure visible behavior.
Some tests build old C versions from Git to compare behavior. They need Git history and Xft development headers.
The temporary files are removed after the tests.
The working tree keeps source and generated output separate:

```text
rust/                  Rust application workspace
src/                   C engine, shared headers and browser resources
tests/                 Regression fixtures and benchmarks
tools/                 Build helpers, hooks and development configuration
build/bin/             Local native executables
build/lib/worminal/     Local frontend/browser modules
build/checks/          Diagnostics and private test runtimes
build/cargo/           Rust compiler cache
build/venv/            Optional Python development environment
```

`python3 manage.py clean` removes compiled output while retaining diagnostics and the development environment.
`manage.py` prevents Python bytecode caches. Local builds and `/usr/local` installs use the same `bin`/`lib`
directories. Keep compiled programs out of the repository root.
Run `python3 manage.py privacy-setup` after cloning to enable the repository's privacy hooks.

Worminal derives from st 0.9.3. See [LICENSE](LICENSE) for copyright and license terms.
