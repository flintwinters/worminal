# Worminal

Worminal is a small X11 terminal based on [st 0.9.3](https://git.suckless.org/st/),
upstream commit `04ce0d643ed17793803e8516f4c9a5b13b93c400`. The upstream license is
in [LICENSE](LICENSE). It uses st's terminal renderer and PTY directly; there is no
webview or JavaScript runtime.

## Build and run

Install a C compiler, `make`, `pkg-config`, and the development headers for X11,
Xft, Fontconfig, and FreeType. On Debian, these are provided by `build-essential`,
`pkg-config`, `libx11-dev`, `libxft-dev`, `libfontconfig-dev`, and `libfreetype-dev`.

```sh
python3 manage.py build
python3 manage.py run
```

The build uses `-O3` by default. `python3 manage.py check` builds and runs native
checks. `python3 manage.py latency` reports median and 95th percentile launch to
first key echo across 20 runs. Both commands need `Xvfb` and `xdotool`, and use a
private Xvfb display so no windows or keystrokes reach your desktop. The latency
probe uses `/bin/cat` to avoid shell startup and does not include Xvfb startup.
`make install` installs the binary and the `st-256color` terminfo entry.
The installed `stterm` package also supplies that entry.

Edit [config.h](config.h) to customize st and rebuild. This first native slice
uses upstream st behavior, including its lack of scrollback. The previous Tauri
implementation remains in Git history.
