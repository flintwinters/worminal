# Worminal

Worminal is a deliberately small terminal. Favor cold launch to usable input and low memory over interface
features; measure both before adding complexity.
Frontend replacements must preserve established user behavior, including removed shortcuts;
plain Ctrl+N belongs to terminal applications, while Ctrl+Shift+N opens a GTK window.

The app is a GTK terminal/browser built on st 0.9.3. A per-user `worminald` service owns ordered tabs, PTYs,
terminal state, and bounded main-screen scrollback; views connect locally or through `--master user@host` over SSH.
Main-screen width changes reflow soft-wrapped text. GTK replicas keep one imported viewport;
reflow and scrollback are not recomputed in the GTK frontend. A tab has one live painting view
and can run with no windows. Each window maps early while its connection worker sends the initial launch request;
font loading and shell startup overlap.
Validated launches overlap connection requests and shell startup with widget construction;
a per-request completion gate keeps GTK callbacks behind completed widget setup.
`manage.py build` compiles Alacritty colors, font family, size, spacing, and scrollback into a generated C header.
Install as the regular user into `~/.local` by default so builds retain that user's Alacritty configuration.
`src/terminal` owns terminal state,
`src/gtk` paints release views, `src/x11` preserves the original renderer, and `src/workspace` connects the service.
Both frontends share paste filtering: bracketed text preserves line endings, while ordinary paste
normalizes CRLF/LF to Enter.
GTK redraws changed rows and cursor damage, including overlapping glyph ink;
blink ticks damage only visible blinking rows, while catalog style changes invalidate their own pixels.
compact rows paint matching background runs before glyphs, with scalar fallback on fractional device grids.
Single-line text draws eligible Pango-shaped horizontal glyph runs directly; other runs retain line drawing.
Multiline text keeps full layouts. Only printable ASCII shaping is cached; Unicode retains on-demand font fallback.
Unchanged frames do not repaint or consume the immediate paint allowance for the first changed keyboard response.
Tab claims can present their first complete response early; cached mapping and rapid selections keep GTK's frame clock.
`tools/build.py` owns builds and installation; `manage.py` owns
checks and latency measurements on private displays.
`manage.py latency-visible` measures real key events to visible echo and fresh/additional window presentation
using distinct launch canvases on private Xvfb; --shell includes real interactive initialization.
It excludes desktop compositing and physical display latency.
`manage.py interaction-perf` measures first ownership claims, cached tab switches, and wheel/page scroll pixels.
`manage.py browser-perf` measures first/additional PTY-to-browser conversions to visible HTTP/file/explorer pixels,
checks HTTP/file keyboard response, and records helper-inclusive memory; snapshots support alternating comparisons.
Browser latency work prioritizes genuine cold opens: `browser-perf --cold-open` includes fresh-process launch,
GTK, service connection and WebKit startup; `--kind` isolates HTTP, file or directory targets.
Use `--established-profile` to distinguish normal cold processes from first-use profile setup;
all private application, helper and service processes stop between trials. Startup reports include CPU and
scheduler queue waits, and milestone logging is opt-in because diagnostic I/O can affect elapsed time.
`--profile-cpu` attributes startup CPU samples without kernel profiling privileges; its timings are diagnostic.
`manage.py render-perf` compares actual release-module painting, exact pixels, CPU and memory on private backends.
The upstream source and license remain in the repository. Keep machine destinations in environment configuration
and use anonymous commit identities. Run `manage.py privacy-setup` in fresh clones; privacy hooks and CI guard
staged content and history.

`src/gtk` is the release GTK 3/Pango-Cairo/WebKitGTK 4.1 frontend for native Wayland and X11. It shares terminal cells,
keys and directory-basename labels with the original frontend. Its Cairo tab strip uses terminal metrics and colors.
One GTK owner per display/workspace accepts separate launches and moves live browser views between local windows;
remote machines share saved URLs only. `worminald` uses the v2 endpoint; browser records have no PTY.
Its shell command converts the calling PTY's ordered tab into a browser. GTK and WebKit never enter the service.
Launchers link GLib only and forward to an existing owner before loading the GTK/Pango frontend module;
the WebKit module loads when a browser view is first claimed. Both modules use local linker scope;
explicit host callbacks carry browser actions across the module boundary. Installed browser modules are
ABI-named so live owners retain compatible lazy loading across upgrades.
New browser views reuse a live renderer for matching HTTP origins or the same local file path;
tabs retain independent navigation and session storage, and the first browser launch stays lazy.
GTK publishes selected terminal directories and window focus through a read-only session D-Bus interface;
`docs/window-state.md` defines the public contract and `manage.py check-window-state` checks it privately.
Explicit X11 window launches use fresh server timestamps for desktop activation;
`manage.py check-window-activation` verifies forwarded and shortcut launches under private Muffin.
The daemon refreshes shell directories after output and every 250 ms, including silent directory changes.
The frontend exports its module/callback interface explicitly; linker collection removes unused owner/parser code.
Hidden terminal views defer cell buffers until first use or frame, and font layouts, input methods,
and blink timers until mapped or used. Empty tab strips defer Pango layouts until catalog refresh.
Service output batching sleeps to its existing deadline rather than adding a fresh interval after partial bursts.
`manage.py check-preview*` and `preview-perf` validate GTK; `build-preview` retains compatibility binary names.
Private Wayland checks use a project-local Weston runtime and matching virtual-seat ABI, without X11 fallback.

Current jobs: prioritize remaining launch latency, while auditing input, rendering, output delivery, and view switching
until further gains require correctness, resource, or compatibility compromises; review remaining compatibility gaps.
Investigate intermittent local workspace loss after idle time; no root cause is confirmed.
Daemon lifecycle, event-loop failures and fatal terminal errors go to syslog for journal diagnosis.
Continue auditing first-frame presentation and peak startup resources after the verified first-window worker overlap.
Browser cold startup is dominated by engine initialization; preserve lazy startup, persistent profiles and
rendering compatibility when revisiting that cost.
`worminal`/`worminald` are GTK/v2; preview build aliases share that workspace and existing browser profiles.
Keep `worminal-x11`/`worminald-x11` and live v1 sessions available for rollback; never migrate or stop them implicitly.
Both daemon builds share explicit `--handin`/`--handoff` ownership transfer via sealed native snapshots and SCM_RIGHTS.
The receiver stages and validates state before commit; the donor then closes PTYs and forgets transferred tabs.
`manage.py check-handoff` covers parser state, queued input, ownership, rollback, and all v1/v2 transfer combinations.
Running daemons that predate the protocol cannot gain it through installation; preserve their live sessions.
Investigate remote Codex scrolling separately from tested wheel modes.

The default browser target is the calling shell directory under worminal:///. Native builds embed the
`file-explorer` submodule's offline HTML in the lazy browser module. The host supplies directory metadata
and bounded read-only text previews only to the embedded page; other pages cannot use these endpoints.
The independent TypeScript workspace retains a folder picker outside Worminal.
Use `manage.py explorer-build`, `explorer-check`, and `explorer-lint` after `npm ci` in the submodule.
