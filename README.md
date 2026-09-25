# Worminal

Worminal is a small X11 terminal based on [st 0.9.3](https://git.suckless.org/st/),
upstream commit `04ce0d643ed17793803e8516f4c9a5b13b93c400`. The upstream license is
in [LICENSE](LICENSE). It uses st's terminal renderer and PTY directly; there is no
webview or JavaScript runtime.

## Build and run

Install Python 3.11+, a C compiler, `make`, `pkg-config`, and the development headers for X11,
Xft, Fontconfig, and FreeType. On Debian, these are provided by `build-essential`,
`pkg-config`, `libx11-dev`, `libxft-dev`, `libfontconfig-dev`, and `libfreetype-dev`.
The management CLI also needs Typer (`requirements-dev.txt`); the compiled
terminal does not.

```sh
python3 manage.py build
python3 manage.py run
```

`build` reads `~/.config/alacritty/alacritty.toml` (or the usual XDG Alacritty
config path), follows `[general].import` in order, and compiles its colors,
`font.size`, `font.offset.y`, and scrolling settings into the binary. The importing file
overrides imported settings. Set
`WORMINAL_ALACRITTY_CONFIG=/path/to/theme.toml` while building to choose another
file. Rebuild after theme changes; Worminal reads no TOML at launch. `run` starts
the already-built binary without rebuilding. Launch `./worminal` directly to
avoid Python startup time.

## Shared tabs

Every `./worminal` launch joins the owner for the same executable build, user,
and X11 `DISPLAY`, opening a new window on a new tab. Rebuilding starts a new
owner on the next launch; existing windows and tabs keep running in the old
owner until closed. Each owner has its own tab list. Every
window shows that list above the terminal and selects a tab independently. Each
label shows the final segment of its tab's working directory. The selected
label is reversed; click a label to select it. Ctrl+T creates a tab,
Ctrl+W closes the selected tab in every window, Ctrl+Tab and Ctrl+Shift+Tab
cycle, Ctrl+1 through Ctrl+8 select by position, Ctrl+9 selects the last tab,
Alt+1 through Alt+9 select exact tab slots, Alt+0 selects slot 10, and Ctrl+N
opens a new window and tab. Ctrl+- decreases the font size; Ctrl++ and Ctrl+=
increase it.

The focused or input-receiving window paints its selected tab. Other windows
selecting that tab keep their terminal pixels until they become live; tab-line
changes still appear. A tab keeps running without a selected window while any
Worminal window remains. Closing the final window ends the owner and all tabs.
Scrollback belongs to the tab, so switching windows preserves its position.
The former `-S` and `-A` prototype flags are retired. Launch options, command,
working directory, and environment are forwarded to the new shell.

The [shared-tab design and proof ledger](docs/shared-tabs.md) records the state
and limits of the X11 proofs.

Supported color settings are primary foreground/background, normal, bright,
explicit dim and indexed colors, cursor colors, and
`draw_bold_text_with_bright_colors`. Both `#RRGGBB` and `0xRRGGBB` are accepted.
Missing colors use Alacritty defaults, including derived dim colors.
`CellForeground` and `CellBackground` cursor references resolve to the primary
colors. Selection colors and opacity retain st behavior.

Scrollback uses `[scrolling].history` (default 10,000 lines) and
`[scrolling].multiplier` (default three lines per wheel step). Use the mouse
wheel or Shift+PageUp/Shift+PageDown to browse history. Typing returns to the
live screen. Alternate-screen programs do not add to scrollback. In that screen,
wheel events go to applications that enable mouse reporting or xterm's alternate
scroll mode. History rows
are allocated as output arrives, so the configured limit does not reserve all
row storage at startup.

Alacritty's `font.size` sets the Xft font's point size, and `font.offset.y` adds
to its cell height at build time; negative offsets make rows more compact.
When rows are shorter than the font, glyphs can extend into adjacent rows.
Worminal still uses its configured Xft font, so exact pixel heights can differ
from Alacritty's renderer.

The build uses `-O3` by default. `python3 manage.py check` builds and runs native
checks. `python3 manage.py proof` runs those checks and every private-display
proof in `tests/proof_*.py`, including isolated KWin and remote Cinnamon.
Use `python3 manage.py proof plasma` or `python3 manage.py proof cinnamon`
to run one managed proof. `python3 manage.py latency` reports median and 95th percentile launch to
the first key queued by X and then echoed across 20 runs.
`python3 manage.py latency-shell` measures when your interactive shell reads
that input. Worminal maps a blank, provisional window before loading fonts, so X can queue early
keystrokes until the terminal processes them. Tests and benchmarks use a private Xvfb
display, so no windows or keystrokes reach your desktop. Checks and benchmarks
need `Xvfb`, `xdotool`, and the XTest development library (`libxtst-dev` on
Debian). The `/bin/cat` probe excludes shell startup; both exclude Xvfb startup.
Worminal advertises `xterm-256color` so applications such as micro recognize
modified Home and End keys. `make install` installs the binary.
It also installs a desktop launcher and scalable icon showing a light pink
tilde on a black circle. The same icon is embedded in the X11 window for panels;
run `python3 manage.py icon` after editing `worminal.svg` to regenerate the
checked-in `icon.h` (requires Inkscape and Pillow).
Edit [config.h](config.h) for other st settings and rebuild. The previous Tauri
implementation remains in Git history.
