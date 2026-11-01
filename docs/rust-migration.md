# Rust migration readiness

The development release launcher and daemon are Rust. GTK terminal and browser
modules remain C. The active `/usr/local` installation has not been replaced.
This checkpoint is not approval to cut over the complete application.

## Usable Rust terminal slice

Run `./worminal-rust` from the project directory. Use `./worminal-rust --attach`
to view existing local v2 sessions without creating another shell, or
`./worminal-rust -e COMMAND ...` to launch a command.

The native Rust app supports terminal windows and tabs, keyboard/IME input,
clipboard and selection, scrolling, zoom, resizing, and reconnecting to the
workspace daemon. Closing a window leaves its shells running; closing a tab
with Ctrl+W or Ctrl+Shift+W closes that terminal. Ctrl+T / Ctrl+Shift+T opens
a tab, Ctrl+Shift+N opens a window, and Ctrl+Tab cycles tabs. Plain Ctrl+N
is sent to the terminal application.

Browser views and SSH are not wired into this executable yet. Existing browser
records appear as placeholders; use `worminal` for those workflows. D-Bus and
the lightweight owner/module integration are also pending. `/usr/local` remains
unchanged. This slice uses the same local v2 workspace and leaves v1 untouched.

Internal maintenance: `manage.py build-rust-app` builds the native executable;
`manage.py check-rust-app` checks real PTY input, tabs/windows, clipboard, resize,
shell survival, idle repaint across daemon replacement and native Wayland.

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
- Rust GTK input, windows, clipboard, IME and the local workspace client are
  available in `worminal-rust`. D-Bus, activation parity, SSH and the lazy
  owner/module integration remain pending. The release frontend is still C.
- Rust browser module, versioned loader, generation pinning and security:
  not ported. Existing C ABI, resources, profiles and security regressions
  remain authoritative.
- Private staged installation: `manage.py check-install` verifies artifact
  bytes, modes, native CLI/SSH input, daemon executable path and resident
  frontend/browser bytes under `.checks/install-stage/usr/local`.
- Final production publication and live-session cutover: pending the GUI
  ports, complete regression/resource audit, verified recovery artifacts,
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
