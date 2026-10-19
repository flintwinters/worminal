<p align="center">
  <img src="assets/worminal.svg" alt="" width="88">
</p>

# Worminal

**A small X11 terminal. Tabs that outlive your windows.**

Worminal keeps your shells in a per-user service, so you can close every window and return to your tabs later.
Open another window on the same machine, or connect to a remote master over SSH: your tabs travel with the service.

Built on [st 0.9.3](https://git.suckless.org/st/), with native X11 rendering and a C core.
The priorities are fast launch to usable input, low memory use, and a small interface.

- **Persistent, shared tabs.** Windows share an ordered tab list and select tabs independently.
- **Local windows, remote shells.** Connect to a remote service with `worminal --master user@host`.
- **Scrollback that follows the tab.** Bounded history and soft-wrap reflow when you resize.
- **Your Alacritty theme, compiled in.** Reuse colors, fonts, spacing, and scrollback settings.
- **Everyday conveniences.** Clickable URLs, clipboard support, mouse-wheel history, and font-size shortcuts.

The default launcher targets X11. A separate GTK/WebKit preview adds native Wayland and browser tabs;
its measured startup and memory overhead is substantially higher. The default remains unchanged.
Remote Codex scrolling remains an investigation area beyond the tested terminal mouse modes.

## Try it

You need Python 3.11+, a C compiler, `pkg-config`, and development headers for
X11, Xft, Fontconfig, and FreeType. On Debian or Ubuntu:

```sh
sudo apt install build-essential pkg-config python3-venv libx11-dev libxft-dev libfontconfig-dev libfreetype-dev
```

From the repository root:

```sh
python3 -m venv .venv
. .venv/bin/activate
python3 -m pip install -r requirements-dev.txt
python3 manage.py build
./worminal
```

Python dependencies are for the management tools; the compiled terminal runs directly.
`python3 manage.py run` also launches the existing binary without rebuilding.

To install both `worminal` and `worminald`, the manual, desktop launcher, and icon:

```sh
sudo .venv/bin/python manage.py install
```

Use `--prefix /your/prefix` for another installation location; `DESTDIR` stages an installation for packaging.
`python3 manage.py uninstall` removes installed files using the same prefix and staging settings.
The build honors `CC`, `PKG_CONFIG`, `CPPFLAGS`, `CFLAGS`, and `LDFLAGS`; `CFLAGS` defaults to `-O3`.
`PREFIX` defaults to `/usr/local`, and `MANPREFIX` defaults to `$PREFIX/share/man`.

## Combined terminal/browser preview

Install `libgtk-3-dev libwebkit2gtk-4.1-dev`, then build the separate preview:

```sh
python3 manage.py build-preview
./worminal-preview
./worminal-preview --surf https://example.org
GDK_BACKEND=wayland ./worminal-preview
```

The preview starts `worminald-preview` on an isolated v2 endpoint. Existing `worminald` sessions continue running.
For a remote workspace, install both preview executables remotely and use `--master user@host`.
`--attach` opens existing preview tabs without creating another shell. Closing a window preserves its service tabs.

Ctrl+Shift+T opens a terminal, Ctrl+Shift+B asks for a browser URL, Ctrl+Shift+N opens another window,
and Ctrl+Shift+W closes the active tab. Ctrl+Tab/Shift+Ctrl+Tab and Alt+1 through Alt+9 switch tabs.
Browser tabs provide Back, Forward, Reload/Stop and a location entry (Ctrl+L). User popup links open as independent
tabs; scripts cannot retain `window.opener`. Cookies and local storage stay in the local XDG `worminal-preview`
profile. A peer's navigation updates the saved link without replacing an already open local page.

Run `python3 manage.py check-preview`, `check-preview-keys`, `check-preview-wayland`, and `check-preview-qtile`
for isolated checks. Wayland checks require Weston and forbid X11 fallback; on Debian, the optional
`setup-preview-wayland` command downloads a runtime into `.checks` without installing system packages.
`preview-perf` compares the original and preview, including WebKit helper memory.
See [preview validation and limitations](docs/browser-integration.txt) before considering a launcher switch.

## Keep your workspace running

Each launch opens a new window and tab. The local `worminald` service starts on demand and owns the shells,
terminal state, and scrollback. Closing a window leaves its tabs running; closing a tab ends it in every window.
Tab labels show the last segment of each tab's working directory.

To keep the shells on another machine, install Worminal there too, make sure `worminald` is in the remote
account's `PATH`, then connect:

```sh
worminal --master user@host
```

Windows connected to the same master share its tabs. Use ordinary `ssh` inside a tab to reach further machines;
those machines need only SSH. Launch options, command, working directory, and environment are forwarded to the
new shell.

A tab has one live painting view at a time: the focused or input-receiving window. Other windows showing that tab
retain their terminal pixels until they become live, while their tab bars continue to update.

To stop a service **and end all its tabs**:

```sh
worminald --stop                 # Local master
ssh user@host worminald --stop   # Remote master
```

### Keyboard and mouse

| Action | Shortcut |
| --- | --- |
| New tab | Ctrl+T |
| Close selected tab everywhere | Ctrl+W |
| Next / previous tab | Ctrl+Tab / Ctrl+Shift+Tab |
| Select tab 1–8 / last tab | Ctrl+1–8 / Ctrl+9 |
| Select exact slot 1–8 / last / slot 10 | Alt+1–8 / Alt+9 / Alt+0 |
| New window and tab | Ctrl+N |
| Smaller / larger font | Ctrl+- / Ctrl++ or Ctrl+= |
| Browse scrollback | Mouse wheel or Shift+PageUp / Shift+PageDown |
| Copy selected text | Ctrl+Shift+C |

Click a tab label to select it. Typing returns scrollback to the live screen.
In applications that capture the mouse, hold Shift while dragging to select text, release the mouse button,
then press Ctrl+Shift+C to copy.

HTTP and HTTPS links underline on hover and open in your **local** browser via `xdg-open`, even for remote shells.
Links can wrap across visible rows. OSC 52 clipboard sequences also update the viewing host's clipboard.

## Bring your theme

At build time, Worminal reads `alacritty.toml`, `alacritty.yml`, or `alacritty.yaml` from
`~/.config/alacritty` (respecting the XDG config directory), preferring TOML. Imports are applied in order;
the importing file overrides imported settings. To choose a different file:

```sh
WORMINAL_ALACRITTY_CONFIG=/path/to/theme.toml python3 manage.py build
```

**Rebuild after changing your theme.** Worminal compiles settings into the binary and reads no config file at launch.
Other st settings live in [src/config.h](src/config.h).

| Setting | Behavior |
| --- | --- |
| Colors | Primary, normal, bright, explicit dim, indexed, and cursor colors |
| `draw_bold_text_with_bright_colors` | Controls bright colors for bold text |
| `font.normal.family` | Xft font family; override for one launch with `-f` |
| `font.size` | Font point size |
| `font.offset.y` | Extra cell height; negative values make rows more compact |
| `font.glyph_offset.y` | Moves glyphs within the grid without moving cursor or selection backgrounds |
| `scrolling.history` | Maximum history lines; default 10,000 |
| `scrolling.multiplier` | Lines per wheel step; default 3 |

Colors accept `#RRGGBB` and `0xRRGGBB`. Missing colors use Alacritty defaults, including derived dim colors.
`CellForeground` and `CellBackground` cursor references resolve to the primary colors.
Selection colors and opacity retain st behavior. Xft font metrics can differ from Alacritty's renderer;
glyphs start at the cell bottom before the glyph offset is applied, and overlapping rows repaint behind them.

Resizing the window or font reflows soft-wrapped main-screen text and history. History storage grows as output
arrives, up to the configured limit. Alternate-screen applications manage their own resizing and do not add to
history; wheel events go to applications that enable mouse reporting or xterm's alternate scroll mode.
Worminal advertises `xterm-256color` for application compatibility.

## Development

The root [manage.py](manage.py) is the entrypoint for builds, checks, and measurements.
Run `python3 manage.py --help` for available commands. Builds use `-O3` by default.

| Path | Responsibility |
| --- | --- |
| [src/terminal](src/terminal) | Terminal state and scrollback |
| [src/x11](src/x11) | Windows, input, and rendering |
| [src/workspace](src/workspace) | Service, client connections, and wire protocol |
| [tools](tools) | Build configuration and management helpers |
| [tests](tests) | Regression tests, display proofs, and benchmarks |
| [assets](assets) | Desktop integration and terminfo |
| `build/` | Generated headers, objects, and test binaries |

### Checks and measurements

```sh
python3 manage.py check-headless  # Native, service, theme, and privacy tests without X11
python3 manage.py check           # Local regression suite, including private-display checks
python3 manage.py lint            # Text limits and Clang static analysis
python3 manage.py latency         # Launch to first echoed input, using /bin/cat
python3 manage.py latency-shell   # Launch to input received by your interactive shell
```

Display checks and launch benchmarks use a private Xvfb display, keeping test windows and keystrokes off your
desktop. They need `Xvfb`, `xdotool`, and XTest development headers (`libxtst-dev` on Debian).
Latency reports median and 95th percentile across 20 runs, excluding Xvfb startup; the `/bin/cat` probe also
excludes shell startup. Windows map before fonts load so X can queue early keystrokes.

Lint requires `clang-tidy`. It enforces 120-character lines and a 1,000-line file limit, with existing larger
files capped at their recorded lengths until split.

`python3 manage.py proof` runs the regression suite and all private-display proofs, including Qtile, KWin,
and remote Cinnamon. Use `proof plasma` or `proof cinnamon` for one managed proof.
The remote proof requires `WORMINAL_CINNAMON_HOST=user@host` in the environment.
The [shared-tab proof ledger](docs/shared-tabs.md) records earlier X11 proofs.

`python3 manage.py master-perf user@host` measures full-frame SSH tab-switch traffic using an isolated service.
Set `WORMINAL_MASTER_DAEMON` to a remote daemon path to compare builds without touching existing tabs.
After editing [assets/worminal.svg](assets/worminal.svg), run `python3 manage.py icon` to regenerate the embedded
window icon; this needs Inkscape and Pillow.

### Contributing

After cloning, enable the repository's privacy hooks and anonymous commit identity:

```sh
python3 manage.py privacy-setup
python3 manage.py check-privacy
```

Keep machine destinations in environment configuration. Commit hooks check staged content, commit messages,
and identities; push hooks and CI scan reachable history. Use `check-privacy --history` to run that scan locally.
Reports show locations and categories without matched values; pattern checks cannot guarantee complete coverage.
Require the Privacy CI job in branch rules to enforce checks even when hooks are bypassed.
Create merge commits locally with the anonymous identity: server-generated merges can fail the history check.

## Origins and license

Worminal is a fork of st 0.9.3, upstream commit `04ce0d643ed17793803e8516f4c9a5b13b93c400`.
See [LICENSE](LICENSE) for copyright and license terms. The previous Tauri implementation remains in Git history.
