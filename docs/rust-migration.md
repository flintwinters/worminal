# Application structure

Rust runs the application. C implements the terminal parser and screen state. The UI uses GTK 3, Pango/Cairo and
WebKitGTK 4.1 on Wayland and X11. Run `worminal` normally, or `worminal --attach` to open existing tabs without
starting another shell. The separate `worminal-rust` executable is for testing new builds.

## Code

- `src/terminal` parses terminal output and handles UTF-8, cells, modes, scrollback, reflow, selection and snapshots.
  `engine.h` defines its C API. Each terminal has its own instance. Rust manages the handles and copies screen rows
  for rendering. C does not manage processes, file descriptors or GUI objects.
- `src/daemon` and `src/workspace` manage shells, PTYs, sockets, SSH, saved workspaces and daemon upgrades.
  Shared protocol and snapshot definitions are in `src/workspace`.
- `src/frontend` handles windows, drawing, keyboard/mouse input, tabs and D-Bus. It receives screen rows from the
  daemon; it does not parse terminal output or recalculate scrollback.
- `src/browser` handles WebKit, permission prompts, downloads, local-file access and browser controls. Shared
  scripts, HTML and module definitions are in `src/gtk`.
- `src/launcher` forwards requests to an existing GUI process before loading GTK. WebKit loads only when needed.

The launcher, frontend and browser communicate through versioned C APIs. They pass handles and callbacks rather
than Rust containers. Each browser view keeps using the module that created it. New views can use updated code
while existing views keep their page state within an owner. Modules share a single browser profile manager.
Automatic owner replacement reloads browser pages while transferring cookies; see `window-updates.md`.

## Sessions and upgrades

The daemon keeps shells alive when windows close. It saves terminal directories and browser URLs so it can reopen
them after a restart. Restart recovery starts fresh shells; it does not recover running programs or scrollback.

A live daemon upgrade transfers PTYs and terminal state to another daemon without restarting shells. The receiving
daemon must validate the state and descriptors before committing. It must also start its own process session, so
exiting the sender cannot kill transferred shells. Before commit, failures leave the sender responsible for its
sessions. After commit, it must close its descriptors without terminating those sessions.

## Old C code

The old C frontends, daemon and application support code have been removed. Git history is the rollback source;
do not keep copies of old executables. Existing v1 processes are not stopped or migrated automatically. Rolling
back a live daemon requires a compatible PTY transfer, not a shell restart.

Some tests compare behavior with the old C version. `tools/reference.py` retrieves that code from Git and compiles
it in a temporary directory under `build/`. Some old tests include source files directly, so the script also adds
temporary links for those files. It removes the links and build directory when the test process exits.
`manage.py clean` removes links left by interrupted tests. Normal application builds do not retrieve old code.
These tests need the Git history and Xft headers.

## Build and test

Use `manage.py` for development tools and native executables to run the app.

| Path | Contents |
| --- | --- |
| `build/bin` | Local executables |
| `build/lib/worminal` | Frontend and browser modules |
| `build/checks` | Test logs, images and private display files |
| `build/cargo` | Rust compiler output |
| `build/venv` | Optional Python environment |

`clean` removes compiled output but keeps diagnostics and the Python environment. Installation uses the same
`bin`/`lib` structure under `/usr/local`.

`check` runs the full test suite on private displays. `check-engine` compares parser behavior and checks malformed
input, snapshots and memory safety. `check-rust-frontend` compares C and Rust rendering pixels on X11 and Wayland.
`check-rust-app` tests the actual GUI and shells. `check-rust-browser` tests browser controls, file access, module
updates, downloads, media playback, permissions, TLS rejection and recovery after a renderer crash.

`check-handoff` and `check-rust-upgrade` test PTY transfers and failures during upgrades.
`prove-preview-regressions` rebuilds code with fixes removed and checks that the tests detect the bugs.
`check-install` tests a staged installation; `check-install --installed` tests the files under `/usr/local`.
`inventory` records running processes, windows, tabs, PTYs and installed file hashes without copying executables.

All GUI tests use private Xvfb or Weston displays. They never send input to the desktop.

## Measurements

The migration's tests passed before installation. The cutover kept the three tabs, three PTYs and all nine running
shell/application process IDs. The later cleanup also passed the full suite, rendering comparisons and installed
SSH/browser checks. Logs are in `build/checks`.

In two HTTP browser trials, Rust used about 5.4–5.5% more total PSS memory than C: 203,402 versus 192,965 KiB for the
first browser, and 265,637 versus 251,782 KiB after opening another. This includes browser helpers, the daemon and
shells. Two trials do not establish a general bound or equal CPU use. Raw results are in
`build/checks/rust-release-browser-perf.json`. Parser checks also showed higher CPU use than the old parser and
record new measurements in `build/checks/engine-resources.json`.
