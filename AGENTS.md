# Worminal

Worminal is a deliberately small terminal. Favor cold launch to usable input and low memory over interface
features; measure both before adding complexity.

The app is an X11-native fork of st 0.9.3. A per-user `worminald` service owns ordered tabs, PTYs, terminal state,
and bounded main-screen scrollback; X11 processes connect locally or through `--master user@host` over SSH. Main-
screen width changes reflow soft-wrapped text. A tab has one live painting view and can run with no windows. Each
window maps early, then loads fonts while the service starts its shell. `manage.py build` compiles Alacritty
colors, font family, size, spacing, and scrollback into a generated C header. `src/terminal` owns terminal state,
`src/x11` paints views, and `src/workspace` connects views to the service. Compact rows repaint backgrounds before
glyphs. `tools/build.py` owns native builds and installation; `manage.py` checks and measures latency on private Xvfb.
The upstream source and license remain in the repository. Keep machine destinations in environment configuration
and use anonymous commit identities. Run `manage.py privacy-setup` in fresh clones; privacy hooks and CI guard
staged content and history.

`src/gtk` is the GTK 3/Pango-Cairo/WebKitGTK 4.1 preview for native Wayland and X11. It shares terminal cells,
keys and directory-basename labels with the original frontend. Its Cairo tab strip uses terminal metrics and colors.
One GTK owner per display/workspace accepts separate launches and moves live browser views between local windows;
remote machines share saved URLs only. `worminald-preview` uses the isolated v2 endpoint; browser records have no PTY.
Its shell command converts the calling PTY's ordered tab into a browser. GTK and WebKit never enter the service.
`manage.py build-preview`, `check-preview*`, and `preview-perf` own preview work.

Current jobs: validate preview parity and reduce its measured launch/memory overhead before considering launcher
replacement; review independent popup-link semantics and remaining compatibility gaps. Keep the original launcher
and live v1 services available for rollback; investigate remote Codex scrolling separately from tested wheel modes.
