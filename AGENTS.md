# Worminal

Worminal is a deliberately small terminal. Favor cold launch to usable input and low memory over interface
features; measure both before adding complexity.

The app is a GTK terminal/browser built on st 0.9.3. A per-user `worminald` service owns ordered tabs, PTYs,
terminal state, and bounded main-screen scrollback; views connect locally or through `--master user@host` over SSH.
Main-screen width changes reflow soft-wrapped text. A tab has one live painting view and can run with no windows. Each
window maps early, then loads fonts while the service starts its shell. `manage.py build` compiles Alacritty
colors, font family, size, spacing, and scrollback into a generated C header. `src/terminal` owns terminal state,
`src/gtk` paints release views, `src/x11` preserves the original renderer, and `src/workspace` connects the service.
GTK redraws changed rows and cursor damage, including overlapping glyph ink; compact rows paint backgrounds before glyphs.
Unchanged frames do not repaint or consume the immediate paint allowance for the first changed keyboard response.
`tools/build.py` owns builds and installation; `manage.py` owns
checks and latency measurements on private displays.
`manage.py latency-visible` measures real key events to visible echo and fresh/additional window presentation
using private Xvfb pixels; it excludes desktop compositing and physical display latency.
The upstream source and license remain in the repository. Keep machine destinations in environment configuration
and use anonymous commit identities. Run `manage.py privacy-setup` in fresh clones; privacy hooks and CI guard
staged content and history.

`src/gtk` is the release GTK 3/Pango-Cairo/WebKitGTK 4.1 frontend for native Wayland and X11. It shares terminal cells,
keys and directory-basename labels with the original frontend. Its Cairo tab strip uses terminal metrics and colors.
One GTK owner per display/workspace accepts separate launches and moves live browser views between local windows;
remote machines share saved URLs only. `worminald` uses the v2 endpoint; browser records have no PTY.
Its shell command converts the calling PTY's ordered tab into a browser. GTK and WebKit never enter the service.
Terminal launches link GTK/Pango only; the WebKit module loads when a browser view is first claimed.
`manage.py check-preview*` and `preview-perf` validate GTK; `build-preview` retains compatibility binary names.

Current jobs: reduce GTK launch/memory overhead and review remaining compatibility gaps.
`worminal`/`worminald` are GTK/v2; preview build aliases share that workspace and existing browser profiles.
Keep `worminal-x11`/`worminald-x11` and live v1 sessions available for rollback; never migrate or stop them implicitly.
Both daemon builds share explicit `--handin`/`--handoff` ownership transfer via sealed native snapshots and SCM_RIGHTS.
The receiver stages and validates state before commit; the donor then closes PTYs and forgets transferred tabs.
`manage.py check-handoff` covers parser state, queued input, ownership, rollback, and all v1/v2 transfer combinations.
Running daemons that predate the protocol cannot gain it through installation; preserve their live sessions.
Investigate remote Codex scrolling separately from tested wheel modes.
