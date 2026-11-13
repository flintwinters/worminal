# Worminal

## Motivation and judgement

Worminal is a deliberately small terminal. Its long-term direction is a distributed operating system that feels present
everywhere at once: machines are appendages and resources of one trusted system, not places the user goes. Tailscale
provides host identity and reachability for now; resources may keep their own protocols. Favor low memory use over interface features; measure before adding
complexity. Preserve established terminal behavior and application shortcuts. Plain Ctrl+N belongs to terminal
applications; Ctrl+Shift+N opens a window. Ctrl+Shift+A selects screen and retained scrollback for Ctrl+Shift+C;
Ctrl+A sends Control-A to applications. Keep the upstream st attribution and license intact.
Use existing modules. Keep each shared rule or definition in one place. Enforce state rules where the state changes.
For bugs with subtle causes, add a test that fails without the fix and explain the reason beside the code.

## Architecture

The maintained application is Rust above a small C terminal engine, with GTK 3, Pango/Cairo and WebKitGTK 4.1 on
native Wayland and X11. `src/terminal/engine.h` defines the C API; Rust bindings generate from it.
Each terminal has its own C instance. C handles parsing, cells, UTF-8, modes, scrollback/reflow, selection, redraw
deadlines and serialization. It does not manage processes, descriptors, GUI objects or application policy.
OSC 8 links are 16-bit cell ids into a bounded per-terminal URI table; ids never leave the daemon: ROW_LINKS
packets carry each row's URIs inline, and snapshots drop them. Clicks open only http(s) or type `worminal://input`
text (newline executes) into the same tab. The Rust wrapper manages C handles and screen buffers, and normalizes clipboard paste in place.
OSC 99 metadata/payload events go only to capable controlling views; those views assemble bounded title/body
notifications and deliver them asynchronously on their desktop, including for remote tabs. Click targets
retain weak window ownership and use the existing tab-selection and platform activation paths.
The frontend owns its ordered keyboard mappings in Rust. Shell defaults live with Rust PTY creation;
browser defaults live with Rust target resolution. C headers retain only engine snapshot and module ABIs:
`src/protocol` owns wire constants and framing, `src/workspace` owns recovery/handoff formats, and `src/gtk`
contains module/D-Bus definitions and browser scripts/templates. The Rust crates under `src` contain the launcher, daemon, frontend, browser
and wrappers.

The per-user v2 daemon owns ordered tabs, PTYs and bounded scrollback. Views connect locally or through SSH
`--master user@host`. A terminal has one live painting view and can outlive every window. Frontend replicas import
one viewport; they never parse terminal output or recompute history/reflow. Browser records have no PTY.
`worminald api` is the headless agent interface over Worp; see `docs/automation.md` and `docs/worp.md`.
`src/protocol::worp` owns shared transport-independent commands/events for every client, plus unary
request/reply conveniences; local sockets and SSH bind them to the existing v2 stream protocol.
Device enrollment is explicit per authority user, shared across scopes and persisted independently of tabs;
`docs/devices.md` defines membership and caller-side availability. Never add heartbeats or block the daemon on probes.
`worminald install-on-remote` bootstraps new compatible headless nodes over SSH; see `docs/remote-install.md`.
It must refuse existing installations/workspaces and preserve state. Version sync is the separate
`worminald upgrade-remote`: views run it once per host when they connect, and it updates whichever side has the older
Cargo version, pushing or pulling the daemon with atomic replacement so the running daemon's watch hands live shells to
the new file. It needs the same CPU, sudo and /usr/local on both sides, and never touches a v1 workspace.
Inspection never takes control or starts a daemon; one-shot input preserves the existing view/controller.
The daemon atomically saves ordered terminal directories and browser URLs, scoped by protocol/workspace under
XDG_STATE_HOME. Restart recovery creates fresh shells with the one-shot WORMINAL_RESTORE=1 marker, never replays
commands or restores processes/scrollback. Real shell startup must respect that marker.

Rust owns bounded queues, controller checks and process identities. Queue complete frames before flushing;
unchanged encoded rows/catalog state must not generate traffic. Negotiated row moves and cell spans preserve
older peers; synchronized redraw retains its deadline behavior. SSH directions advance independently under
congestion. Spawn local daemons and SSH without GUI fork hooks, in their own sessions, after validating the display. A failed launch
must not create orphan tabs. The same-user GUI broker authenticates peers before accepting launch data or
descriptors.

Ownership transfers stage and validate sealed native snapshots and SCM_RIGHTS before commit. Receivers establish
an independent session before accepting PTYs. Track local children separately from transferred processes and pin
identities before signalling. After commit, donors close descriptors without destroying transferred shells or
saving empty recovery recipes. Failed upgrades retain healthy owners; never restart live shells for rollback.
Existing v1 processes must never be stopped or migrated implicitly.

The lightweight launcher links GLib only and forwards before loading graphics. Frontend and browser use distinct,
versioned C module ABIs with opaque handles and explicit callbacks; no Rust containers cross modules. Each browser
view pins its creating generation; generations share a persistent profile manager. Keep code resident for async
callbacks and release unused contexts. Never let concurrent owners write the same browser profile. Candidate
owners/profiles are isolated from the stable installation. Browser moves preserve live DOM within the local owner;
remote workspaces share saved URLs only. Updated owners stage automatic window replacement; see `docs/window-updates.md`. Older owners need one manual restart.

Painting preserves matching background runs, overlapping glyph/cursor damage, fractional-grid fallback and Unicode
font fallback. Cache printable ASCII shaping only. Views defer unused buffers/layouts/input methods/timers.
The Cairo tab strip uses terminal metrics/colors and the tab order reported by the daemon. Browser file labels
decode the filename; web tabs retain document titles. D-Bus directory arrays retain original filesystem bytes.
`docs/window-state.md` defines that read-only public contract. X11 activation uses fresh server timestamps;
Wayland activation remains compositor policy.

Browser URL resolution preserves explicit URLs, prefers existing paths and otherwise defaults bare hosts to HTTPS.
`--new-browser-tab URL|FILE` requires a target; `./` and `../` disambiguate missing local files.
Selected local documents have bounded, read-only file/directory APIs through a private per-view credential held by
a top-frame-only script. Subframes and other navigated documents have no capability. Native error/find chrome is
Rust; its shared scripts/templates remain under `src/gtk`. Renderer recovery retains the last valid URI because
WebKit can clear it after termination. Release Rust borrows before entering reentrant GTK/WebKit operations.
The independent TypeScript `file-explorer` workspace opens its HTML artifact through the native browser CLI.
Its changes take effect only on explicit reload/reopen; do not add live updates. Native builds do not embed it.
Parent explorer build commands supply palette/font inputs; standalone builds retain their own defaults.

## Build, test and install

`manage.py` is strictly internal build/test/install/maintenance tooling. Product workflows use native executables.
Rust bindings require libclang plus the matching Clang compiler and builtin headers; libclang alone is insufficient.
`check-rust-lint` (part of `check`) enforces rustfmt, a Rust file-length cap and warning-free Clippy (rustup
component `clippy`). Lint policy lives once in the workspace `Cargo.toml`; long-function exceptions use `#[expect]`.
Configuration is runtime, never compiled in: `src/config` is the only resolver (`worminald config --json` for tools),
layering `worminal.yaml` over an Alacritty config; see `docs/config.md`. Defaults in `assets/font.yaml` use bundled
Atkinson Hyperlegible Mono; installation includes both font styles and their OFL license. Scrollback capacity is
fixed by a process's first engine; other settings apply to new processes/windows.
Builds and installs auto-detect a headless host (no GTK/WebKit/Pango via pkg-config, or `WORMINAL_HEADLESS=1`; `0` forces graphics) and then build/install only `worminald` and the manual. Cargo output and generated files belong in
`build/`; native executables use `build/bin` and modules use `build/lib/worminal`, matching installed prefixes.
`tools/paths.py` owns runtime/diagnostic path decisions. Diagnostics belong in `build/checks`; optional Python
virtualenvs use `build/venv`. `clean` preserves diagnostics/virtualenvs while removing compiled output.
Analysis/dependency configuration lives under `tools`. Management suppresses Python bytecode caches.
Never test under /tmp or inject test input into the desktop.
Private test runtimes must outlive their GUI and service owners; await owner exit before removing their files.
Publication checks require Unix ownership semantics. Update workflows automatically use project-local tmpfs
on UID-remapping filesystems, preserve diagnostics, then unmount; this also requires passwordless sudo.
`manage.py` drops desktop display variables. `manage.py check` runs the full private-display suite. Native Wayland
checks use project-local Weston with matching virtual-seat ABI and no X11 fallback. Prepare optional private
Muffin/Weston through their setup commands. Performance checks measure private software presentation, not desktop
compositing or physical display latency. Launch latency is not a migration acceptance target.

The old C applications are no longer maintained or installed. Roll back code from Git; do not keep executable
backups. Some tests still need the old C code. `tools/reference.py` gets it from Git, builds it under `build/`,
and creates temporary source links for those tests. It removes the files and links when the test process exits.
Normal builds do not use this code. `manage.py clean` removes links left by interrupted tests.
Keep behavior checks when changing tests. Some tests check fixes by removing them and requiring a failure.
Keep user profiles and diagnostic records when cleaning up code.

`manage.py setup-check-runtime` prepares private browser audio and Weston without changing desktop packages;
update workflows run it automatically. New runtimes stay under `build/dependencies` across diagnostic cleanup.
`manage.py update` builds the current checkout, installs to /usr/local, then verifies the installed artifacts on
private displays; it does not run the test suite (`update --check` does, as auto-update does). Installation and any
private tmpfs mounts use sudo; checks run as the user.
`manage.py auto-update --timer install` schedules a daily 03:00 systemd user timer: it builds and checks the tip of
`origin/main` in a private clone under `build/auto-update`, installs to /usr/local only if everything passes, and
skips unchanged tips. It shares the checkout update pipeline and needs passwordless sudo for the install step.

The active installation is `/usr/local/bin/worminal`, with modules in `/usr/local/lib/worminal`. Install a tested
regular-user build with `sudo -n python3 manage.py install --prefix /usr/local --skip-build`; do not install fixes
into ~/.local. `check-install` validates a staged installation; `check-install --installed` checks installed files,
modes, SSH/browser operation and actual module paths on private displays. Running owners retain mapped code; v2 owners hand off by themselves, v1 daemons are never touched.
`manage.py running` classifies live processes against the installed files (`update` prints it); run it before claiming a change is active.
`manage.py update` is the one-command flow and needs no manual preparation: it takes a per-checkout lock, releases tmpfs left by aborted runs,
and `setup-check-runtime` builds a missing/stale explorer (`npm ci` as needed) and prepares audio and Weston under `build/dependencies`.
If it still stops, fix the cause in that flow rather than around it. Inventory commands record
process IDs, tabs, windows and file hashes without copying executables. Keep destinations in environment configuration
and preserve the contributor's configured Git identity.

## Current investigations

Investigate intermittent local workspace loss after idle time; its root cause is not confirmed.
Remote daemons older than 2fc0825 can resend full frames for idle controlled (including hidden) tabs at
link rate; upgrade them before metered use. With negotiated focused frames, hidden tabs send nothing until revealed. Treat remote Codex scrolling separately from tested wheel modes.
Daemon lifecycle/event-loop/fatal terminal errors go to syslog for diagnosis. Audit input, rendering, output delivery
and view switching until further gains require correctness, resource or compatibility compromises.
