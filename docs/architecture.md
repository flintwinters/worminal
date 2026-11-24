# Architecture and behavior contracts

Read the relevant sections before changing terminal behavior, ownership, transport or GUI integration.

## Terminal behavior and judgement

Worminal is a deliberately small terminal. Its long-term direction is a distributed operating system that feels
present everywhere at once: machines are appendages and resources of one trusted system, not places the user goes.
Tailscale provides host identity and reachability for now; resources may keep their own protocols. Favor low memory
use over interface features; measure before adding complexity. Preserve established terminal behavior and
application shortcuts. Plain Ctrl+N belongs to terminal applications; Ctrl+Shift+N opens a window. Every
window, including plain `worminal` launches, shows every attached master's tabs without launching its shells. Ctrl+Shift+A
selects screen and retained scrollback for Ctrl+Shift+C; Ctrl+A sends Control-A to applications. Plain middle click
toggles browser-style autoscroll (pointer distance from the click sets speed; any press, key or focus loss ends it)
through the same dispatch as the wheel, so mouse-reporting and alternate-screen applications scroll too;
Shift+middle click pastes the primary selection. Ctrl+scroll over a terminal cycles tabs in the focused pane,
wrapping like Ctrl+Tab (up selects the previous tab; down selects the next), before application mouse handling.
Dropping local files onto a terminal pastes shell-quoted absolute paths at its current cursor, separated by spaces,
through the ordinary bracketed-paste path without submitting input.
Keep the upstream st attribution and license intact. Use existing
modules. Keep each shared rule or definition in one place. Enforce state rules where the state changes. For bugs
with subtle causes, add a test that fails without the fix and explain the reason beside the code.

## Engine and module boundaries

The maintained application is Rust above a small C terminal engine, with GTK 3, Pango/Cairo and WebKitGTK 4.1 on
native Wayland and X11. `src/terminal/engine.h` defines the C API; Rust bindings generate from it. Each terminal has
its own C instance. C handles parsing, cells, UTF-8, modes, scrollback/reflow, selection, redraw deadlines and
serialization. It does not manage processes, descriptors, GUI objects or application policy. OSC 8 links are 16-bit
cell ids into a bounded per-terminal URI table; ids never leave the daemon: ROW_LINKS packets carry each row's URIs
inline, and snapshots drop them. Clicks open ordinary links (http(s), file, ...) with xdg-open.
Visible path tokens beginning with `/` or `~/` become file URLs on the frontend machine, where `~` expands to
the local user's home directory. Only `worminal://`
links are restricted, to typing `worminal://input` text (newline executes) into the same tab. The Rust wrapper manages C handles and screen buffers, and normalizes clipboard paste in place. OSC 99
metadata/payload events go only to capable controlling views; those views assemble bounded title/body notifications
and deliver them asynchronously on their desktop, including for remote tabs. OSC 7501 agent status (working) and OSC
9;4 progress (normal or indeterminate) set the engine's `MODE_BUSY` bit, carried in the catalog mode word to every
view; the daemon clears it when the shell regains the foreground. Busy tabs get a static gray background; live
background or stopped shell jobs get cyan, observed by the daemon through shell child process groups. Click targets
retain weak window ownership and use the existing tab-selection and platform activation paths. The Rust engine
wrapper owns key, mouse, paste and wheel encoding; the daemon applies it to semantic Worp input with live modes,
and views use it for local effects and older daemons. The frontend owns GUI shortcuts.
Shell defaults live with Rust PTY creation; browser defaults live with Rust target resolution. C headers retain only
engine snapshot and module ABIs: `src/protocol` owns wire constants and framing, `src/workspace` owns
recovery/handoff formats, and `src/gtk` contains module/D-Bus definitions and browser scripts/templates. The Rust
crates under `src` contain the launcher, daemon, frontend, browser and wrappers.

## Workspaces and transport

The per-user v2 daemon owns ordered tabs, PTYs and bounded scrollback. Views connect locally or through SSH
`--master user@host`. A terminal has one live painting view and can outlive every window. Windows are pure views
with one link, to the local daemon, and own no tabs. The local daemon owns attached masters (`src/daemon/src/masters.rs`):
it holds each master link, lists the master's tabs in its own catalog under local ids with their host, relays
frames and input, shows on the master exactly what local views show, persists the set and the records beside the
recovery recipes, and reconnects with backoff, prompting for a password only on an explicit attach. So a new window
knows master tabs at once, and they never move the selection unless the user asked for that master. A new window
starts with the split of the window it opened from. Frontend replicas import
one viewport; they never parse terminal output or recompute history/reflow. Browser records have no PTY. `worminald
api` is the headless agent interface over Worp; see `docs/automation.md` and `docs/worp.md`. The executable alone
must teach an agent everything: `worminald --help` indexes `api help [schema|session]`, `config`,
`install-on-remote` and `upgrade-remote`. Parsers, generated help and API help search share the command/option
registry in `src/workspace/src/cli_schema.rs`; search enumerates every declared group, spelling and alias without
a separate command list. Detailed references use the same providers as native command help, without loading graphics.
`api_help.rs` and `src/config/src/reference.rs` own the API/configuration texts; tests check them against real
replies and the resolver. `src/protocol::worp` owns shared transport-independent
commands/events for every client, plus unary request/reply conveniences; local sockets and SSH bind them to the
existing v2 stream protocol. Device enrollment is explicit per authority user, shared across scopes and persisted
independently of tabs; `docs/devices.md` defines membership and caller-side availability. Never add heartbeats or
block the daemon on probes. `worminald install-on-remote` bootstraps new compatible nodes over SSH, adding the
`worminal` command when the target has the graphics stack; see `docs/remote-install.md`. It must refuse existing
installations/workspaces and preserve state. Version sync is also exposed as `worminald upgrade-remote`. Every GUI
master connection first runs the shared
update bootstrap immediately after SSH authentication, before config sync or any bridge/workspace operation.
The same authenticated stream becomes the bridge only after synchronization succeeds; failures disconnect the view.
It updates whichever side has the older Cargo version, pushing or pulling the
daemon with atomic replacement so the running daemon's watch hands live shells to the new file. It needs the same
CPU, sudo and /usr/local on both sides, and never touches a v1 workspace. It compares only the Cargo version, so
bump it with any change remote daemons must receive; equal versions never upgrade.
`worminald --list-masters` passively lists successful outbound view routes and their live kernel leases, independently
of explicit device enrollment. History starts at a valid catalog and survives disconnects; it never drives probes.
Every tab's environment carries `WORMINAL_DAEMON`, the serving worminald's path: an agent learns it is in a tab
and where to start discovery.
Inspection never takes control or starts a daemon; one-shot input preserves the existing view/controller. Agent
reads expose bounded history/batches and opaque observation revisions; deferred waits never block the service.
Output cursors reference retained physical cells and report gaps across eviction/layout changes; bounded command
snapshots and completion waits follow explicit OSC 133 boundaries. Atomic paste/key requests validate once before
queueing. Optional OSC 133 shell markers report command boundaries/status without inferring completion from idle
output. Input guards bind the workspace instance and foreground process identity; they do not certify processing.
The daemon atomically saves ordered terminal directories and browser URLs, scoped by protocol/workspace under
XDG_STATE_HOME. Restart recovery creates fresh shells with the one-shot WORMINAL_RESTORE=1 marker, never replays
commands or restores processes/scrollback. Real shell startup must respect that marker.

## Process and transfer ownership

Rust owns bounded queues, controller checks and process identities. Queue complete frames before flushing; unchanged
encoded rows/catalog state must not generate traffic. Negotiated row moves and cell spans preserve older peers;
synchronized redraw retains its deadline behavior. SSH directions advance independently under congestion. Spawn
local daemons and SSH without GUI fork hooks, in their own sessions, after validating the display. A failed launch
must not create orphan tabs. GUI SSH authentication uses Worminal’s isolated askpass dialog; background reconnects
never prompt, back off until a catalog arrives, and retain disconnection feedback. The same-user GUI broker
authenticates peers before accepting launch data or descriptors. Launcher and frontend share the broker's display
identity; automatic and explicit X11 join the same owner when no Wayland display is configured.

Ownership transfers stage and validate sealed native snapshots and SCM_RIGHTS before commit. Receivers establish an
independent session before accepting PTYs. Track local children separately from transferred processes and pin
identities before signalling. After commit, donors close descriptors without destroying transferred shells or saving
empty recovery recipes. Failed upgrades retain healthy owners; never restart live shells for rollback. Existing v1
processes must never be stopped or migrated implicitly.

## GUI and browser lifetime

The lightweight launcher links GLib only and forwards before loading graphics. Frontend and browser use distinct,
versioned C module ABIs with opaque handles and explicit callbacks; no Rust containers cross modules. Each browser
view pins its creating generation; generations share one profile manager per owner, held only while the owner has
browser tabs: its network process keeps its heap until the manager finalizes, so closing the last browser tab
ends it (with session cookies, like quitting a browser) once views and downloads release it. The owner keeps the
profile lock throughout. Keep code resident for async callbacks and release unused contexts. Never let concurrent
owners write the same browser profile. Candidate
owners/profiles are isolated from the stable installation. Browser moves preserve live DOM within the local owner;
remote workspaces share saved URLs only. Updated owners warm a provisional GUI, refresh a frozen descriptor-backed
snapshot, and acknowledge commit before the donor retires; see
`docs/window-updates.md`. Older owners need one manual restart.

## Rendering and panes

Painting preserves matching background runs, overlapping glyph/cursor damage, fractional-grid fallback and Unicode
font fallback. Cache printable ASCII shaping only. Views defer unused buffers/layouts/input methods/timers. Dragging
a tab onto a pane edge (or Alt+Shift+arrows) splits the window (`layout.rs` tree of pane ids and divider ratios, `app_split.rs` widgets; per window). The layout is
remembered between runs by tab position in the daemon's order, never by tab id (`app_layout.rs`, `layout_state.rs`,
`docs/layout-restore.md`). Each pane is its own Cairo strip over a notebook holding only its tabs; a one-pane window is the
ordinary case. The daemon only learns the shown set (each pane's selected tab): `Focus` shows one tab, `Pane`
(feature `PANES`) adds one, and `Resize` never hides neighbours. A browser tab has no frames, so it splits freely, but the daemon's `Focus` is exclusive: while terminals are visible beside it, claiming the browser presents those terminals instead of focusing the browser. Ctrl+Shift+X gathers all tabs into the focused
pane. The Cairo tab strip uses terminal metrics/colors and the tab order reported by the daemon. Browser file labels
decode the filename; web tabs retain document titles. D-Bus directory arrays retain original filesystem bytes.
`docs/window-state.md` defines that read-only public contract. X11 activation uses fresh server timestamps; Wayland
activation remains compositor policy.

## Side panel and bookmarks

Ctrl+Shift+B (or P) opens a collapsible side panel beside the panes (`panel.rs`, glue in `app_panel.rs`). It is a Cairo list
in the terminal's font and palette with its own tabs; a tab implements `PanelTab` (`panel_tabs.rs`: title, rows, press,
and optionally an add prompt), and Bookmarks is the first, so further lists add an implementation. The panel is a sibling of the pane area, never an overlay
child (GTK drag hit testing would hide drop targets), and its widget exists only while open. Bookmarks are the
user's `$XDG_CONFIG_HOME/worminal/bookmarks.yaml` (`workspace/src/bookmarks.rs`; `docs/bookmarks.md`): terminal
directories with an optional SSH master, URLs, and servers (`host:`, validated SSH endpoints), never commands. Opening a
server starts `sh -ic 'trap : INT; ssh "$1"; exec "$SHELL"'` in a new local tab with the endpoint as `$1`: ssh gets
its own foreground process group (so the daemon labels the tab and does not bookmark it as a local directory), and an
interrupted or failed ssh leaves the user's shell and its error, not a closed tab. It follows the `devices.yaml` rules through the
shared `yamlfile.rs`: bounded no-follow reads, a non-blocking lock around read-modify-publish, atomic replacement,
and a damaged file reported and kept. Windows re-read it on focus; there is no watching or polling. The tab strip's
star and Ctrl+Shift+D toggle through `app_bookmarks.rs`.

## Browser capabilities

Browser URL resolution preserves explicit URLs, prefers existing paths and otherwise defaults bare hosts to HTTPS.
`--new-browser-tab URL|FILE` requires a target; `./` and `../` disambiguate missing local files. Selected local
documents have bounded, read-only file/directory APIs through a private per-view credential held by a top-frame-only
script. Subframes and other navigated documents have no capability. Native error/find chrome is Rust; its shared
scripts/templates remain under `src/gtk`. Renderer recovery retains the last valid URI because WebKit can clear it
after termination. Release Rust borrows before entering reentrant GTK/WebKit operations. Local HTML tools open through the native
browser CLI; their changes take effect only on explicit reload/reopen, and native builds embed none of them.
