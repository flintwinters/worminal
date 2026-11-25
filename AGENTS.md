# Worminal

## Motivation and judgement

Worminal is a deliberately small terminal moving toward one trusted distributed system: machines are resources,
not places the user goes. Tailscale currently supplies host identity and reachability. Favor low memory use over
interface features, preserve established terminal behavior and application shortcuts, and measure before adding
complexity. Keep the upstream st attribution and license intact.

Use existing modules. Keep shared rules authoritative in one place and enforce state invariants where state
changes. For subtle bugs, keep a regression that fails without the fix and explain the cause beside the code.
Read the relevant [architecture and behavior contracts](docs/architecture.md) before changing a subsystem.

## Architecture

Rust owns the launcher, daemon, frontend, browser, configuration and transport above a small C terminal engine.
`src/terminal/engine.h` defines the engine API; generated Rust bindings follow it. C owns parsing, cells,
scrollback/reflow, selection and serialization; it never owns processes, descriptors, GUI objects or app policy.
The frontend imports daemon viewports; it never parses output or recomputes history. Windows are pure views
with one link to the local daemon; that daemon owns master links and lists their tabs, so every window shows them. `src/protocol` owns the
screen and input vocabulary, so alternative frontends need no engine; the daemon encodes semantic input with
live modes. `worminald attach [--master HOST]` exposes the Worp stream.
[`worminald terminal`](docs/terminal-view.md) presents the workspace in a local or SSH terminal.
GTK 3, Pango/Cairo and WebKitGTK 4.1 support native Wayland and X11.

The per-user v2 daemon owns ordered tabs and PTYs independently of windows. Local sockets and SSH bind the same
Worp commands/events from `src/protocol::worp`. Rust owns bounded queues, controller checks and process identities.
Never add heartbeats or block the daemon on probes. Inspection is passive; atomic agent input validates before
queueing. Device membership is explicit, persisted independently of tabs and scoped to an authority user.

Live ownership transfers validate sealed snapshots and descriptors before commit, preserve shell identities and
establish independent receiver sessions. Failure retains a healthy owner; rollback never restarts live shells.
Restart recovery restores directory/URL recipes into fresh shells, never commands, processes or scrollback.

The lightweight GLib launcher forwards before loading graphics. Frontend/browser modules have distinct versioned
C ABIs; Rust containers never cross them. Browser generations retain their callbacks and share profile ownership.
GUI updates commit a fresh frozen snapshot after provisional preparation; ordinary window edits never invalidate it.
Candidate owners/profiles remain isolated from the stable installation. Preserve established browser profile and
broker identities, including names containing `preview`. Release Rust borrows before reentrant GTK/WebKit calls.

`src/config` is the only runtime configuration resolver. Native executables must teach agents their public CLI,
API and configuration contracts without repository access. `src/workspace/src/cli_schema.rs` defines CLI spellings
once for parsers, help and search.
`manage.py` is internal development tooling only.
Master views finish version sync immediately after SSH authentication, before workspace access.
Remote version sync compares Cargo versions: bump the version when remote daemons need a runtime change; equal
versions never upgrade. Release daemons build `--portable`, linking musl statically so sync works across
distributions; default native builds link glibc. Sync failures notify the desktop. Tests target the current Rust app
and C engine, without historical application builds.
Full checks enforce reviewed daemon CPU and network performance ceilings; never raise them automatically.

Read [development.md](docs/development.md) for build/test/install workflows and [testing.md](docs/testing.md) for
private test workers. Use only project-local test runtimes and private displays; never inject into the desktop or
test under `/tmp`. Await all private owners before removing their files. Preserve diagnostic records and profiles.
Build artifacts belong in `build/`; diagnostic paths belong to `tools/paths.py`. Keep installation destinations in
environment configuration and preserve the contributor's Git identity. Roll back from Git, not executable backups.

The public landing page in `website/` uses `UI.md`'s Gruvbox pane chrome with natural document scrolling. Keep core
product copy visible without interaction and navigation crawlable. `manage.py` builds and checks self-contained
HTML independently of native dependencies. GitHub Actions publishes `build/site` to Pages; `tools/site.py` owns
the canonical URL and embeds the shared logo, styles and interactions.

Public contracts and subsystem details:

- [Agent interface](docs/automation.md), [Worp](docs/worp.md), [devices](docs/devices.md), [bookmarks](docs/bookmarks.md), [layout restore](docs/layout-restore.md).
- [Configuration](docs/config.md), [notifications](docs/notifications.md), [window state](docs/window-state.md).
- [Remote installation and master history](docs/remote-install.md), [upgrades](docs/remote-upgrade.md),
  [window replacement](docs/window-updates.md).

## Current big tasks

Audit input, rendering, output delivery and view switching until further gains require correctness, resource or
compatibility compromises. Measure private software presentation; do not infer physical display latency.

Remote daemons older than 2fc0825 can resend idle full frames at link rate; upgrade them before metered use.
Negotiated focused frames suppress hidden-tab traffic. Use syslog lifecycle/event-loop/fatal terminal diagnostics
when investigating reliability, and preserve the evidence across cleanup.
