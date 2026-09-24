# Worminal

Worminal is a small X11 terminal based on [st 0.9.3](https://git.suckless.org/st/),
upstream commit `04ce0d643ed17793803e8516f4c9a5b13b93c400`. The upstream license is
in [LICENSE](LICENSE). It uses st's terminal renderer and PTY directly; there is no
webview or JavaScript runtime.

## Build and run

Install Python 3.11+, a C compiler, `make`, `pkg-config`, and the development headers for X11,
Xft, Fontconfig, and FreeType. On Debian, these are provided by `build-essential`,
`pkg-config`, `libx11-dev`, `libxft-dev`, `libfontconfig-dev`, and `libfreetype-dev`.

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

Supported color settings are primary foreground/background, normal, bright,
explicit dim and indexed colors, cursor colors, and
`draw_bold_text_with_bright_colors`. Both `#RRGGBB` and `0xRRGGBB` are accepted.
Missing colors use Alacritty defaults, including derived dim colors.
`CellForeground` and `CellBackground` cursor references resolve to the primary
colors. Selection colors and opacity retain st behavior.

Scrollback uses `[scrolling].history` (default 10,000 lines) and
`[scrolling].multiplier` (default three lines per wheel step). Use the mouse
wheel or Shift+PageUp/Shift+PageDown to browse history. Typing returns to the
live screen. Alternate-screen programs do not add to scrollback. History rows
are allocated as output arrives, so the configured limit does not reserve all
row storage at startup.

Alacritty's `font.size` sets the Xft font's point size, and `font.offset.y` adds
to its cell height at build time; negative offsets make rows more compact.
When rows are shorter than the font, glyphs can extend into adjacent rows.
Worminal still uses its configured Xft font, so exact pixel heights can differ
from Alacritty's renderer.

The build uses `-O3` by default. `python3 manage.py check` builds and runs native
checks. `python3 manage.py latency` reports median and 95th percentile launch to
the first key queued by X and then echoed across 20 runs.
`python3 manage.py latency-shell` measures when your interactive shell reads
that input. Worminal maps a blank, provisional window before loading fonts, so X can queue early
keystrokes until the terminal processes them. These commands use a private Xvfb
display, so no windows or keystrokes reach your desktop. Checks and benchmarks
need `Xvfb`, `xdotool`, and the XTest development library (`libxtst-dev` on
Debian). The `/bin/cat` probe excludes shell startup; both exclude Xvfb startup.
`make install` installs the binary and the `st-256color` terminfo entry.
It also installs a desktop launcher and scalable icon showing a light pink
tilde on a black circle.
The installed `stterm` package also supplies that entry.

Edit [config.h](config.h) for other st settings and rebuild. The previous Tauri
implementation remains in Git history.
