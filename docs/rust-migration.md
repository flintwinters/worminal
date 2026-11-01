# Rust migration readiness

The development release launcher, daemon and GTK/WebKit modules are Rust above
the shared C terminal engine. `worminal-rust` retains a separate candidate GUI
namespace and browser profile. The active `/usr/local` installation has not been replaced.
The installation and cutover gates below remain incomplete.

## Usable Rust application

Run `./worminal-rust` from the project directory. Use `./worminal-rust --attach`
to view existing local v2 sessions without creating another shell, or
`./worminal-rust -e COMMAND ...` to launch a command.
Use `./worminal-rust --master user@host` for a remote workspace through SSH.
Use `./worminal-rust --surf URL|FILE` (or `--browser`) for a browser window.
The native `--new-browser-tab URL|FILE` command converts its calling terminal tab.

The native Rust app supports terminal windows and tabs, keyboard/IME input,
clipboard and selection, scrolling, zoom, resizing, and reconnecting to the
workspace daemon. Closing a window leaves its shells running; closing a tab
with Ctrl+W or Ctrl+Shift+W closes that tab. Ctrl+T opens a tab of the current
kind; Ctrl+Shift+T always opens a terminal. Ctrl+Shift+N opens a window, and Ctrl+Tab cycles tabs. Plain Ctrl+N
is sent to the terminal application.
The Cairo tab strip retains terminal font metrics, inverted selection and browser
colors. It clips oversized tabs and keeps the selected suffix visible. Local
browser tabs show decoded filenames; web tabs retain document titles.

Separate native launches join one Rust owner per display/workspace. Browser
tabs retain their live DOM when moved between its windows and across daemon
replacement. Ctrl+K opens the location entry; Ctrl+F finds text, Ctrl+R reloads,
and Alt+Left/Right navigates history. Downloads use the native save dialog.
Selected local HTML retains the established top-frame-only credential and
bounded read APIs; independent documents and subframes receive no capability.

The candidate uses `worminal-rust` under XDG data/cache directories so it cannot
write an installed C owner's live browser profile. Each profile has an exclusive
owner lock. Browser module generations share one live data manager, while views
pin their original code and independent navigation/session storage. The installed
profile remains untouched; selecting it requires closing its existing browser
owners during the final cutover. The default development executable now selects
the stable profile; final publication remains pending. This app uses the same v2
workspace and leaves v1 untouched.

Internal maintenance: `manage.py build-rust-app` builds the native executable;
`manage.py check-rust-app` checks real PTY input, tabs/windows, clipboard, resize,
shell survival, idle repaint across daemon replacement and native Wayland.
It also checks the container-free frontend ABI, listener ownership on rejection,
GLib-only launcher dependencies and forwarding without loading the module again.
`manage.py check-rust-browser` checks real browser navigation, owner forwarding,
live DOM through daemon upgrades, module generations, shared session cookies,
local-document security, explicit file reload, downloads and native Wayland frames.

The original C `worminal-x11` and `worminald-x11` remain v1 rollback programs.
`build/worminald-c-v2` builds the shared-engine C service for reverse transfers
and compatibility checks. Neither build selection nor private tests migrate
production workspaces.

- One C terminal parser, opaque instance API, generated FFI and safe owning
  wrapper: `manage.py check-engine` covers baseline differential behavior,
  snapshots, damaged rows, events, malformed state, sanitizers and forbidden
  engine imports.
- Rust v2 protocol, persistence, PTYs, process identities, bounded service loop
  and SSH bridge: `manage.py check-rust-daemon`, `check-rust-workspace`, and
  the routine `check` suite exercise C clients against Rust.
- Manual transfer, automatic replacement, failed receiver, cancellation and
  reverse rollback: `manage.py check-rust-handoff` and `check-rust-upgrade`.
  Compiled counterfactuals in `prove-preview-regressions` guard ownership.
- GTK/WebKit behavior with the Rust daemon: `manage.py check-rust-daemon-x11`
  and `check-rust-daemon-wayland` use private displays. X11 also retains live
  browser DOM through upgrades and C rollback. These use C GUI modules.
- Rust imported viewports and Pango/Cairo painting are implemented in
  `rust/frontend`; `manage.py check-rust-frontend` compares exact pixels
  with the C frontend on private X11 and Wayland, including Unicode fallback,
  clipping, cursor shapes, palette styles, selection and fractional grids.
  Selection remains in C and borrows Rust cells only during each call.
  The same check compares Cairo tab-strip pixels, including browser colors,
  Unicode, zoom, hidden prefixes and oversized selection clipping.
- Rust GTK input, windows, clipboard, IME, local/SSH workspace clients and
  owner forwarding and the public window-state D-Bus interface are available in
  `worminal-rust`. X11 activation is checked against private Muffin, including
  fresh server timestamps; its compiled counterfactual fails without the fix.
  Prepare that optional runtime through `manage.py setup-window-activation`.
  The candidate lazily loads `frontend-rust-v2.so` through the generated ABI in
  `src/gtk/rust_frontend_module.h`; the default launcher selects the Rust frontend.
  Immutable module flags select stable versus candidate profiles. The C GTK peer
  remains available as `build/worminal-gtk-c` for compatibility checks.
  Only the owner loads GTK/Pango/Cairo, and only a browser claim loads WebKit.
- Rust browser module, versioned loader and generation pinning are implemented.
  `src/gtk/rust_browser_module.h` owns the incompatible opaque ABI; both sides
  generate bindings. Browser chrome/scripts remain shared resources. Complete
  security/media/permission parity and final profile publication still need the
  full regression audit. Browser ABI 2 carries palette background changes.
- Private staged installation: `manage.py check-install` verifies artifact
  bytes, modes, native CLI/SSH input, daemon executable path and resident
  frontend/browser bytes under `.checks/install-stage/usr/local`.
- Final production publication and live-session cutover: pending the complete
  regression/resource audit, verified recovery artifacts,
  live inventory and privately verified upgrade/reverse-transfer paths for
  the final artifacts.

## Measured resource cost

On 2026-10-31, `manage.py frame-perf --compare .checks/c-baseline/build`
compared the preserved C daemon with the Rust release daemon on the same
40-write PTY fixture. Both delivered 81 complete frames.

| Measurement | Preserved C | Rust |
| --- | ---: | ---: |
| Daemon PSS | 289 KiB | 980 KiB |
| Daemon RSS | 1,796 KiB | 2,924 KiB |
| Output-to-frame median / p95 | 1.93 / 2.13 ms | 1.89 / 1.99 ms |
| Daemon CPU counter change | 0 ms | 0 ms |

The Rust daemon increased measured PSS by 691 KiB. CPU counters have 10 ms
resolution; this short fixture does not establish equal CPU use. These are
daemon-only measurements, not whole-application or browser resource results.
Recorded comparison output is in `.checks/rust-default-resources.log`;
`frame-perf` writes its latest raw samples to
`.checks/frame-output-latency-{baseline,candidate}.json`.
The engine check separately records parser CPU/RSS in `.checks/engine-resources.json`.
Current engine checks still show higher parser CPU than the pre-extraction baseline.
Repeat comparisons with the final frontend/browser ports before cutover;
launch latency is not a migration optimization requirement.

The Rust-default checkpoint passed `manage.py check` (141 existing tests, one
intentional v1 browser skip, both C GTK compatibility runs, 19 Rust application
tests and 7 Rust browser tests). The subsequent browser run passed all 8 tests,
including exact pixels for a converted tab's custom background. `check-install`
passed against the staged installation; production publication remains pending.
Logs are `.checks/rust-release-{full-check,app,browser,install}.log`.

Two alternating private HTTP trials with `browser-perf --kind http --trials 2
--compare .checks/c-baseline/build` measured the following median PSS, including
the frontend, WebKit helpers and workspace service/shells regardless of parenting:

| State | Preserved C | Rust |
| --- | ---: | ---: |
| First browser | 192,965 KiB | 203,402 KiB |
| Additional browser | 251,782 KiB | 265,637 KiB |

This fixture shows roughly 5.4–5.5% higher PSS; two trials do not establish a
general memory bound or equal CPU use. Raw samples and exact process counts are
in `.checks/rust-release-browser-perf.json`. The preserved recovery manifest's
18 artifacts still match their hashes. Broader browser parity and production
inventory/cutover verification remain separate gates.
