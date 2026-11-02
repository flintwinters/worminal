# Rust application and C engine

The installed GTK/v2 application uses Rust for the launcher, workspace service, PTYs, GTK frontend and WebKit
integration. A small C engine owns terminal semantics. The application retains GTK 3, Pango/Cairo and WebKitGTK 4.1
on native Wayland and X11. Run `worminal` normally; `worminal --attach` opens existing tabs without creating shells.
`worminal-rust` remains an isolated development owner/profile, built on demand.

## Source ownership

- `src/terminal/engine.h` defines the engine ABI. C owns UTF-8/escape parsing, cells, modes, scrolling/reflow,
  selection, terminal events and snapshots. Rust generates bindings, owns each handle and copies viewport rows.
- `rust/daemon` and `rust/workspace` own process identities, bounded service queues, persistence, SSH and transfers.
  Shared wire/snapshot definitions remain in `src/workspace`; there is no maintained C service implementation.
- `rust/frontend` owns windows, Pango/Cairo painting, input, tabs and D-Bus publication. Replicas never parse output
  or recompute scrollback. `src/gtk` retains authoritative module/D-Bus headers and shared browser resources.
- `rust/browser` owns native browser policy/chrome. ABI-named modules pin per-view generations; generations share
  a live profile manager. The stable profile uses `worminal-preview`; the candidate uses separate `worminal-rust`
  data/cache paths. The independent file explorer remains a TypeScript workspace opened as a local HTML document.

The frontend and browser module ABIs use opaque handles and explicit callbacks, never Rust containers. Only a GUI
owner loads GTK/Pango; only a browser claim loads WebKit. Renderer recovery retains the last nonempty URI because
WebKit clears it after process termination; normal reloads retain their semantics. Reentrant WebKit calls must not
hold Rust state borrows across synchronous notifications.

## Legacy retirement and rollback

The legacy C GTK/X11 applications, daemon and adapters have been removed from the maintained tree. Normal builds
and installation produce only the Rust application/modules and C engine. Installation removes obsolete C executable
and module filenames; it never deletes browser profiles or signals running processes.

**Git history is the code rollback mechanism. No recovery release or executable backup is retained.** Reconstruct
old code from its commit when necessary. Existing v1 processes are never migrated or stopped implicitly. Any live
v2 daemon rollback must use a compatible, verified ownership transfer; never restart shells to roll back code.

Historical parser, rendering, client and daemon comparisons remain reproducible through `tools/reference.py`.
It extracts the pre-retirement reference from Git into a disposable directory under `build/`, exposing temporary
source/executable links only for the check process. Cleanup removes those links and the fixture directory on exit.
`manage.py clean` also removes links left by an interrupted check. These are comparative test fixtures, not shipped
or preserved rollback applications. Normal builds work without reconstructing historical sources; comparative
checks require the referenced Git objects. The original st attribution/license remains intact.

## Verification

`manage.py` is internal tooling; user workflows use native executables. Local builds use `build/bin` and
`build/lib/worminal`, the same prefix layout as `/usr/local`. Diagnostics/private displays live in `build/checks`;
compiler output stays in `build/cargo`. `clean` preserves diagnostics and `build/venv`, not compiled output.

| Command | Coverage |
| --- | --- |
| `build` | Rust application/modules and standalone C engine; no historical application build |
| `check` | Complete private-display, engine, protocol, persistence, upgrade and GUI/browser regression gate |
| `check-engine` | Git-derived parser differential, malformed input/state, sanitizer and safe-wrapper checks |
| `check-rust-frontend` | Exact historical C/Rust terminal/tab-strip pixels on private X11 and Wayland |
| `check-rust-app` | Real shell/SSH input, clipboard, IME, tabs/windows, activation, reconnection and shell survival |
| `check-rust-browser` | Local capabilities, navigation, module generations, live DOM, downloads, media and recovery |
| `check-handoff`, `check-rust-upgrade` | Commit failures, queued input, process ownership and reverse transfer |
| `prove-preview-regressions` | Compiled counterfactuals for non-obvious publication/ownership/activation invariants |
| `check-install` | Private staged installation: hashes, modes, native SSH/browser and actual module identities |
| `check-install --installed` | The same checks against published `/usr/local` artifacts, without publishing |
| `inventory` | Read-only live owners/windows, ordered tabs, PTYs, process identities and installed hashes |

All graphical checks use private Xvfb or project-local Weston; none inject input into the desktop. Native Wayland
checks disable X11 fallback. Private media tests decode MP3/WAV/MP4 into a silent PulseAudio sink. Tests cover
permission allow/deny, invalid TLS, find/link/popup behavior and explicit renderer crash/reload recovery.

## Published migration evidence and measured costs

The migration's final full gate passed 141 existing tests (one inapplicable v1 browser test skipped), both C reference
GUI runs, 19 Rust application tests and 13 Rust browser tests. Published artifacts passed private SSH/browser and
hash/mode/module-path checks. Live cutover recreated three windows while retaining three ordered tabs, three PTYs
and all nine original shell/application process identities. Logs remain under `build/checks/rust-production-*.log`;
inventory/result JSON records are diagnostic evidence, not recovery releases.

The C retirement gate repeats the behavior checks with Git-backed disposable references. Its diagnostics use
`build/checks/retire-c-*.log`. Code copies are removed; user profiles and diagnostic measurements remain.

A 40-write daemon fixture delivered 81 complete frames for both implementations. Preserved measurement data showed
C/Rust daemon PSS of 289/980 KiB and median output-to-frame latency of 1.93/1.89 ms. The short CPU sample had 10 ms
counter resolution and did not establish equal CPU use. The extracted parser still showed higher CPU than the
pre-extraction baseline; engine checks record current samples in `build/checks/engine-resources.json`.

Two alternating private HTTP browser trials measured median total PSS of 192,965/203,402 KiB for the first C/Rust
browser and 251,782/265,637 KiB after an additional browser. This includes frontend/helpers and service/shells
regardless of parenting: approximately 5.4–5.5% higher PSS in that fixture. Two trials do not establish a general
bound or equal CPU use. Raw measurements remain in `build/checks/rust-release-browser-perf.json`; executable snapshots
are not retained. Launch-latency improvement was not a migration requirement.
