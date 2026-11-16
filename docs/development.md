# Development

## Tooling boundaries

`manage.py` is strictly internal build/test/install/maintenance tooling. Product workflows use native executables.
Rust bindings require libclang plus the matching Clang compiler and builtin headers; libclang alone is insufficient.
`check-rust-lint` (part of `check`) enforces rustfmt, a Rust file-length cap and warning-free Clippy (rustup
component `clippy`). Lint policy lives once in the workspace `Cargo.toml`; long-function exceptions use `#[expect]`.

## Runtime configuration

Configuration is runtime, never compiled in: `src/config` is the only resolver (`worminald config --json` for
tools), layering `worminal.yaml` over an Alacritty config; see `docs/config.md`. Defaults in `assets/font.yaml` use
the system DejaVu Sans Mono with Atkinson Hyperlegible Mono only as an installed fallback; no font is packaged.
Scrollback capacity is fixed by a process's first engine; other settings apply to new processes/windows.

## Build outputs

Builds and installs auto-detect a headless host (no GTK/WebKit/Pango via pkg-config, or `WORMINAL_HEADLESS=1`; `0` forces
graphics) and then build/install only `worminald` and the manual. Cargo output and generated files belong in
`build/`; native executables use `build/bin` and modules use `build/lib/worminal`, matching installed prefixes.
`tools/paths.py` owns runtime/diagnostic path decisions. Diagnostics belong in `build/checks`; optional Python
virtualenvs use `build/venv`. `clean` preserves diagnostics/virtualenvs while removing compiled output.
Analysis/dependency configuration lives under `tools`. Management suppresses Python bytecode caches. Never test
under /tmp or inject test input into the desktop.

## Private checks

Private test runtimes must outlive their GUI and service owners;
await owner exit before removing their files. Publication checks require Unix ownership semantics. Update workflows
automatically use project-local tmpfs on UID-remapping filesystems, preserve diagnostics, then unmount; this also
requires passwordless sudo. `manage.py` drops desktop display variables. `manage.py check` runs the full
private-display suite. Native Wayland checks use project-local Weston with matching virtual-seat ABI and no X11
fallback. Prepare optional private Muffin/Weston through their setup commands. Performance checks measure private
software presentation, not desktop compositing or physical display latency.

The retired C applications and their Git-built reference infrastructure are removed. Build and test the current Rust
application and C engine directly; do not reconstruct old applications or require historical Git revisions. Roll
back code from Git rather than keeping executable backups. Keep user profiles and diagnostic records.

## Updates

`manage.py setup-check-runtime` prepares private browser audio, Weston and Muffin without changing desktop packages; update
workflows run it automatically. New runtimes stay under `build/dependencies` across diagnostic cleanup. `manage.py
update` builds the current checkout, installs to /usr/local, then verifies the installed artifacts on private
displays; it does not run the test suite (`update --check` does, as auto-update does). Installation and any private
tmpfs mounts use sudo; checks run as the user. `manage.py auto-update --timer install` schedules a daily 03:00
systemd user timer: it builds and checks the tip of `origin/main` in a private clone under `build/auto-update`,
installs to /usr/local only if everything passes, and skips unchanged tips. It shares the checkout update pipeline
and needs passwordless sudo for the install step.

## Installation and live processes

The active installation is `/usr/local/bin/worminal`, with modules in `/usr/local/lib/worminal`. Install a tested
regular-user build with `sudo -n python3 manage.py install --prefix /usr/local --skip-build`; do not install fixes
into ~/.local. `check-install` validates a staged installation; `check-install --installed` checks installed files,
modes, SSH/browser operation and actual module paths on private displays. Running owners retain mapped code; v2
owners hand off by themselves, v1 daemons are never touched. `manage.py running` classifies live processes against
the installed files (`update` prints it); run it before claiming a change is active. `manage.py update` is the
one-command flow and needs no manual preparation: it takes a per-checkout lock, releases tmpfs left by aborted runs,
and `setup-check-runtime` builds a missing/stale explorer (`npm ci` as needed) and prepares audio and Weston under
`build/dependencies`. If it still stops, fix the cause in that flow rather than around it. Inventory commands record
process IDs, tabs, windows and file hashes without copying executables. Keep destinations in environment
configuration and preserve the contributor's configured Git identity.

## Check entrypoints

Use `python3 manage.py --help` as the command index. The main routes are:

| Command | Purpose |
| --- | --- |
| `build` | Current launcher, daemon and graphics modules, or the daemon alone on headless hosts |
| `check` | Current engine, Rust and private-display regression suites |
| `check-engine` | Sanitized C engine boundaries and Rust wrapper ownership |
| `check-perf` | Enforce reviewed daemon CPU and deterministic network ceilings (also part of `check`) |
| `check-rust-frontend` | Viewport import and frame publication |
| `check-rust-app` | Terminal GUI, keyboard input, module loading and reattachment |
| `check-rust-browser` | Browser controls, local-file capabilities, media and renderer recovery |
| `check-handoff`, `check-upgrade` | Live PTY transfers and failed replacements |
| `prove-regressions` | Require tests to fail when current-code fixes are removed |
| `check-build` | Build freshness, installation layout, cleanup and runtime configuration |
| `check-install` | Staged installation; `--installed` verifies the published files |
| `latency-visible` | Fresh/additional windows and visible input; `--shell` includes shell startup |
| `setup-wayland`, `setup-audio` | Prepare individual private-display/media runtimes |

`worminal-rust` is an isolated candidate launcher with its own broker/profile identity. It remains necessary for
private app checks. The removed `worminal-preview` and `worminald-preview` aliases had no distinct behavior;
use `worminal` and `worminald`. Persistent profile and broker names containing `preview` are established runtime
identities, not build aliases: changing them would split existing owners or lose access to browser profiles.

The superseded `latency` and `latency-shell` pipe-echo tools and binding-only platform smoke test are removed.
Current application checks exercise the graphics bindings; `latency-visible` measures actual presentation.
Recorded measurements remain in `docs/latency-history.txt`, `docs/latency-audit.txt` and `build/checks`.
See [testing.md](testing.md) for Python workers and [history](history/) for earlier architecture records.
