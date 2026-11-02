# Worminal

## Motivation and judgement

Worminal is a deliberately small terminal. Favor low memory use over interface features; measure before adding
complexity. Preserve established terminal behavior and application shortcuts. Plain Ctrl+N belongs to terminal
applications; Ctrl+Shift+N opens a window. Keep the upstream st attribution and license intact.
Use existing modules, keep authoritative decisions in one place, and enforce non-obvious invariants in the operation
that owns the state change. Preserve reproducible failures and verified rationale beside those operations.

## Architecture

The maintained application is Rust above a small C terminal engine, with GTK 3, Pango/Cairo and WebKitGTK 4.1 on
native Wayland and X11. `src/terminal/engine.h` owns the opaque per-instance engine ABI; Rust bindings generate from
it. C owns parsing, cells, UTF-8, modes, scrollback/reflow, selection, redraw deadlines and serialization. It owns no
processes, descriptors, GUI objects or application policy. Safe Rust wrappers own handles and copied frame rows.
`src/workspace` contains authoritative wire/snapshot/policy headers; `src/gtk` contains shared module/D-Bus headers
and browser scripts/templates, not C applications. `rust` owns the launcher, daemon, frontend, browser and wrappers.

The per-user v2 daemon owns ordered tabs, PTYs and bounded scrollback. Views connect locally or through SSH
`--master user@host`. A terminal has one live painting view and can outlive every window. Frontend replicas import
one viewport; they never parse terminal output or recompute history/reflow. Browser records have no PTY.
The daemon atomically saves ordered terminal directories and browser URLs, scoped by protocol/workspace under
XDG_STATE_HOME. Restart recovery creates fresh shells with the one-shot WORMINAL_RESTORE=1 marker, never replays
commands or restores processes/scrollback. Real shell startup must respect that marker.

Rust owns bounded queues, controller checks and process identities. Queue complete frames before flushing;
unchanged encoded rows/catalog state must not generate traffic. Row moves and synchronized redraw retain their
negotiated compatibility/deadline behavior. SSH directions advance independently under congestion. Spawn local
daemons and SSH without GUI fork hooks, after validating the display. A failed launch must not create orphan tabs.
The same-user GUI broker authenticates peers before accepting launch data or descriptors.

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
remote workspaces share saved URLs only. GTK/frontend updates require closing their old owners.

Painting preserves matching background runs, overlapping glyph/cursor damage, fractional-grid fallback and Unicode
font fallback. Cache printable ASCII shaping only. Views defer unused buffers/layouts/input methods/timers.
The Cairo tab strip uses terminal metrics/colors and authoritative catalog order. Browser file labels decode the
filename; web tabs retain document titles. Window-state D-Bus directory arrays retain original filesystem bytes.
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

## Building, checking and publication

`manage.py` is strictly internal build/test/install/maintenance tooling. Product workflows use native executables.
Build as the regular user to compile their Alacritty appearance. Cargo output and generated files belong in
`build/`; native executables use `build/bin` and modules use `build/lib/worminal`, matching installed prefixes.
`tools/paths.py` owns runtime/diagnostic path decisions. Diagnostics belong in `build/checks`; optional Python
virtualenvs use `build/venv`. `clean` preserves diagnostics/virtualenvs while removing compiled output.
Hooks and analysis/dependency configuration live under `tools`; management suppresses Python bytecode caches.
Never test under /tmp or inject test input into the desktop.
`manage.py` drops desktop display variables. `manage.py check` runs the full private-display suite. Native Wayland
checks use project-local Weston with matching virtual-seat ABI and no X11 fallback. Prepare optional private
Muffin/Weston through their setup commands. Performance checks measure private software presentation, not desktop
compositing or physical display latency. Launch latency is not a migration acceptance target.

Legacy C applications have been removed from maintained source and installation. Git history is the only code
rollback source; do not preserve recovery releases or executable copies. `tools/reference.py` reconstructs the
historical regression reference from Git in disposable build directories. Its sources and executable links exist
only during checks and are removed at process exit; `manage.py clean` also removes interrupted fixture links.
Historical C comparisons are test fixtures, never production binaries. Normal builds do not extract them.
Retain behavior assertions and compiled counterfactuals when adapting checks. Installed profiles and diagnostic
inventories are user/runtime data, not code rollback copies; never delete them as part of code retirement.

The active installation is `/usr/local/bin/worminal`, with modules in `/usr/local/lib/worminal`. Publish a verified
regular-user build through `sudo -n python3 manage.py install --prefix /usr/local --skip-build`; do not install fixes
into ~/.local. `check-install` validates a staged installation; `check-install --installed` checks published bytes,
modes, SSH/browser operation and actual module paths on private displays. Running owners retain mapped code;
verify executable/module identities before claiming a change is active. Inventory commands record live state and
hashes without copying releases. Keep destinations in environment configuration and preserve the contributor's
configured Git identity. Privacy hooks guard staged content/history messages, not public commit attribution.

## Current investigations

Investigate intermittent local workspace loss after idle time and reported desktop freezes during master
connections; neither root cause is confirmed. Treat remote Codex scrolling separately from tested wheel modes.
Daemon lifecycle/event-loop/fatal terminal errors go to syslog for diagnosis. Audit input, rendering, output delivery
and view switching until further gains require correctness, resource or compatibility compromises.
