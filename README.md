<p align="center">
  <img src="assets/worminal.svg" alt="" width="88">
</p>

# Worminal

The `file-explorer` submodule is a TypeScript npm workspace building a plain dark,
offline explorer as `file-explorer/dist/index.html`. Run `npm ci` inside the submodule, then
`python3 manage.py explorer-build` or `python3 manage.py explorer-check` from this repository.
`python3 manage.py explorer-lint` runs its pinned Hyperlinter; setup is in the submodule README.
Standalone browsers use a read-only folder picker. Inside Worminal, the independent page uses a generic
read-only directory metadata API. Worminal neither embeds the page nor needs reinstalling when it changes.
The explorer lives at [worminal-file-explorer](https://github.com/flintwinters/worminal-file-explorer).
Use `git submodule update --init --recursive` to initialize it and its pinned Hyperlinter.

**A small GTK terminal and browser. Tabs that outlive your windows.**

Worminal keeps your shells in a per-user service, so you can close every window and return to your tabs later.
Open another window on the same machine, or connect to a remote master over SSH: your tabs travel with the service.

If windows become unresponsive or tabs disappear, inspect `journalctl -t worminald --since today`.
New daemon processes report startup, normal exits, event-loop failures and fatal terminal errors through syslog.
Signal deaths and OOM kills require the system crash/kernel logs. This reporting does not recover lost shells;
an already running daemon keeps its existing behavior until an explicit ownership transfer or its next start.

Built on [st 0.9.3](https://git.suckless.org/st/), with GTK 3/Pango-Cairo rendering, WebKitGTK browser tabs, and a C
core.
The priorities are fast launch to usable input, low memory use, and a small interface.

- **Persistent, shared tabs.** Windows share an ordered tab list and select tabs independently.
- **Local windows, remote shells.** Connect to a remote service with `worminal --master user@host`.
- **Scrollback that follows the tab.** Bounded history and soft-wrap reflow when you resize.
- **Your Alacritty theme, compiled in.** Reuse colors, fonts, spacing, and scrollback settings.
- **Everyday conveniences.** Clickable URLs, clipboard support, mouse-wheel history, and font-size shortcuts.

The small GLib launcher forwards additional windows before loading graphics libraries. The first window loads
GTK on Wayland or X11; WebKit loads when opening a browser. Both modules install under `lib/worminal`.
After validating the display and arguments, windows overlap connection requests and shell startup
with widget construction. Connection callbacks wait until the widgets are ready.
Remote Codex scrolling remains an investigation area beyond the tested terminal mouse modes.

## Try it

You need Python 3.11+, a C compiler, `pkg-config`, and development headers for
GTK 3, WebKitGTK 4.1, X11, Xft, Fontconfig, and FreeType. On Debian or Ubuntu:

```sh
sudo apt install build-essential pkg-config python3-venv libx11-dev libxft-dev libfontconfig-dev libfreetype-dev \
  libgtk-3-dev libwebkit2gtk-4.1-dev
```

From the repository root:

```sh
python3 -m venv .venv
. .venv/bin/activate
python3 -m pip install -r requirements-dev.txt
python3 manage.py build
./worminal
```

Python dependencies are for the management tools; the compiled terminal runs directly.
`python3 manage.py run` also launches the existing binary without rebuilding.

To install `worminal`, `worminald`, and the `worminal-x11`/`worminald-x11` rollback pair, the manual, desktop
launcher, and icon:

```sh
.venv/bin/python manage.py install
```

Use `--prefix /your/prefix` for another installation location; `DESTDIR` stages an installation for packaging.
`python3 manage.py uninstall` removes installed files using the same prefix and staging settings.
The build honors `CC`, `PKG_CONFIG`, `CPPFLAGS`, `CFLAGS`, and `LDFLAGS`; `CFLAGS` defaults to `-O3`.
`PREFIX` defaults to `~/.local`, and `MANPREFIX` defaults to `$PREFIX/share/man`.
Install as your regular user so builds read your Alacritty configuration. Keep `~/.local/bin` on `PATH`.
When switching from a system installation, also update your desktop's default terminal command to
`~/.local/bin/worminal` (using its full absolute path). A hardcoded system command bypasses `PATH`
and loads the old frontend and its compiled font settings even after restarting all windows.

## Terminal and browser

The main build includes terminal and browser tabs:

```sh
python3 manage.py build
./worminal
./worminal --surf https://example.org
GDK_BACKEND=wayland ./worminal
```

Worminal starts `worminald` on the v2 endpoint. Existing v1 sessions remain available through `worminal-x11`;
install builds that rollback frontend and `worminald-x11`. Installation never migrates or stops sessions automatically.
For a remote workspace, install the current release remotely and use `--master user@host`.
The old preview executable names remain build aliases and share the v2 workspace.
`--attach` opens existing tabs without creating another shell. Closing a window preserves its service tabs.
Inside a Worminal terminal, `worminal --new-browser-tab URL|FILE` replaces that terminal with a browser,
ending its shell. A target is required: HTTP(S), `file://` and other supported URLs are preserved;
bare hosts such as `youtube.com` default to HTTPS. Existing paths and explicit `./`, `../`, or absolute paths
become encoded `file://` URLs. Prefix missing dotted filenames with `./` to distinguish them from hosts.
Local files use normal WebKit handling.
Ctrl+K reveals the address entry. Ctrl+F finds page text; Enter and Shift+Enter move between matches.
Escape selects the result and focuses its link for Enter. Repeating Ctrl+F selects the query for replacement.

The file explorer is an independent local HTML application. Run `python3 manage.py explorer-build`,
then open its current `file-explorer/dist/index.html` with `--new-browser-tab`.
From a Worminal terminal, `python3 /path/to/file-explorer/manage.py open` passes that HTML file and
its caller's directory explicitly. Rebuild the explorer and reopen or reload the page to use new output;
Worminal needs no rebuild or reinstall. Native builds require neither the explorer submodule nor npm.
Selected local HTML documents receive `window.worminal.readDirectory("/absolute/path")` for read-only metadata.
Entries are `[base64Name, isDirectory, bytes, modifiedMs]`. Only the selected top-level document,
including its query routes, retains access; subframes and other navigated pages do not.

Ctrl+Shift+T opens a terminal, Ctrl+Shift+N opens another window,
and Ctrl+Shift+W closes the active tab. Ctrl+Tab/Shift+Ctrl+Tab and Alt+1 through Alt+9 switch tabs.
Plain Ctrl+N passes through to the terminal application.
Browser tabs provide Back, Forward, Reload/Stop and a location entry (Ctrl+K). User popup links open as independent
tabs; scripts cannot retain `window.opener`. Cookies and local storage stay in the local XDG `worminal-preview`
profile. A peer's navigation updates the saved link without replacing an already open local page.

Run `python3 manage.py check-preview`, `check-preview-keys`, `check-preview-wayland`, and `check-preview-qtile`
for isolated checks. Wayland checks require Weston and forbid X11 fallback; on Debian/Ubuntu, the optional
`setup-preview-wayland` command downloads a supported Weston 10 or 16 runtime into `.checks` with matching
virtual-seat headers, without installing system packages.
`preview-perf` compares the original and preview, including WebKit helper memory.
See [preview validation and limitations](docs/browser-integration.txt) for measured costs and compatibility constraints.

External applications can read the selected terminal's full working directory and watch changes through the
[session D-Bus window-state interface](docs/window-state.md), independently of window titles on Wayland and X11.

## Keep your workspace running

Each launch opens a new window and tab. The local `worminald` service starts on demand and owns the shells,
terminal state, and scrollback. Closing a window leaves its tabs running; closing a tab ends it in every window.
Tab labels show the last segment of each tab's working directory.

To keep the shells on another machine, install Worminal there too, make sure `worminald` is in the remote
account's `PATH`, then connect:

```sh
worminal --master user@host
```

Windows connected to the same master share its tabs. Use ordinary `ssh` inside a tab to reach further machines;
those machines need only SSH. Launch options, command, working directory, and environment are forwarded to the
new shell.

A tab has one live painting view at a time: the focused or input-receiving window. Other windows showing that tab
retain their terminal pixels until they become live, while their tab bars continue to update.

To stop a service **and end all its tabs**:

```sh
worminald --stop                 # Local master
ssh user@host worminald --stop   # Remote master
```

### Transfer sessions between daemons

Both daemon builds support an explicit, local ownership transfer. Arm a running destination, then ask the
running source to transfer its entire workspace using the same one-time name:

```sh
worminald --handin cutover
worminald-x11 --handoff cutover
worminal --attach
```

Use each daemon's `WORMINAL_SHARED_SOCKET_SCOPE` when transferring between isolated workspaces. These commands
connect to existing services and never start or restart one. **Both running processes must already contain the
handoff implementation.** Replacing a binary on disk does not upgrade a running daemon that predates this protocol.

The receiver appends the source's ordered tabs, assigns destination IDs, and takes sole ownership of their PTYs
and lifecycle. The donor closes its descriptors and forgets those tabs after commit. Shell PIDs, environment,
working directories, foreground jobs, both screens, scrollback, parser state, and pending input survive; the donor
can exit afterward. Linux keeps the original parent relationship until that parent exits, but input, resize,
signalling, and shutdown belong to the receiver. Imported shells use pidfds to prevent signalling a reused PID.
Open views and their stdout/printer streams are not moved; attach a destination view to continue interacting.

The receiver validates a sealed snapshot and duplicated descriptors before commit. Rejection, timeout, or a
connection failure before commit leaves donor sessions in place. Once committed, ownership never automatically
falls back, including if the receiver crashes or its acknowledgement is lost. This is a same-user Linux transfer,
using a versioned native snapshot shared by these two daemon builds, with a limit of 100 tabs and 512 MiB per
transfer. History must fit the destination's compiled capacity. Browser records transfer their saved URLs between
v2 daemons; a v1 receiver rejects any workspace containing browsers without moving its terminals.

Run `python3 manage.py check-handoff` for sanitizer-backed snapshot checks and isolated live transfers in all
four protocol combinations, including queued input and failure rollback. These checks also run in `check-headless`.

### Keyboard and mouse

| Action | Shortcut |
| --- | --- |
| New tab | Ctrl+T |
| Close selected tab everywhere | Ctrl+W |
| Next / previous tab | Ctrl+Tab / Ctrl+Shift+Tab |
| Select tab 1–8 / last tab | Ctrl+1–8 / Ctrl+9 |
| Select exact slot 1–8 / last / slot 10 | Alt+1–8 / Alt+9 / Alt+0 |
| Smaller / larger font | Ctrl+- / Ctrl++ or Ctrl+= |
| Browse scrollback | Mouse wheel or Shift+PageUp / Shift+PageDown |
| Copy selected text | Ctrl+Shift+C |

Click a tab label to select it. Typing returns scrollback to the live screen.
In applications that capture the mouse, hold Shift while dragging to select text, release the mouse button,
then press Ctrl+Shift+C to copy. Ctrl+Shift+V pastes the clipboard; Shift+Insert and middle-click
paste the primary selection. Bracketed paste preserves text line endings; ordinary paste treats
CRLF and LF as one Enter each.

HTTP and HTTPS links underline on hover and open in your **local** browser via `xdg-open`, even for remote shells.
Links can wrap across visible rows. OSC 52 clipboard sequences also update the viewing host's clipboard.

## Bring your theme

At build time, Worminal reads `alacritty.toml`, `alacritty.yml`, or `alacritty.yaml` from
`~/.config/alacritty` (respecting the XDG config directory), preferring TOML. Imports are applied in order;
the importing file overrides imported settings. To choose a different file:

```sh
WORMINAL_ALACRITTY_CONFIG=/path/to/theme.toml python3 manage.py build
```

**Rebuild after changing your theme.** Worminal compiles settings into the binary and reads no config file at launch.
Other st settings live in [src/config.h](src/config.h).

| Setting | Behavior |
| --- | --- |
| Colors | Primary, normal, bright, explicit dim, indexed, and cursor colors |
| `draw_bold_text_with_bright_colors` | Controls bright colors for bold text |
| `font.normal.family` | Font family; override for one launch with `-f` |
| `font.size` | Font point size |
| `font.offset.y` | Extra cell height; negative values make rows more compact |
| `font.glyph_offset.y` | Moves glyphs within the grid without moving cursor or selection backgrounds |
| `scrolling.history` | Maximum history lines; default 10,000 |
| `scrolling.multiplier` | Lines per wheel step; default 3 |

Colors accept `#RRGGBB` and `0xRRGGBB`. Missing colors use Alacritty defaults, including derived dim colors.
`CellForeground` and `CellBackground` cursor references resolve to the primary colors.
Selection colors and opacity retain st behavior. Frontend font metrics can differ from Alacritty's renderer;
glyphs start at the cell bottom before the glyph offset is applied, and overlapping rows repaint behind them.

Resizing the window or font reflows soft-wrapped main-screen text and history. History storage grows as output
arrives, up to the configured limit. Alternate-screen applications manage their own resizing and do not add to
history; wheel events go to applications that enable mouse reporting or xterm's alternate scroll mode.
Worminal advertises `xterm-256color` for application compatibility.

## Development

The root [manage.py](manage.py) is the entrypoint for builds, checks, and measurements.
Run `python3 manage.py --help` for available commands. Builds use `-O3` by default.
Browser UI checks include native local-media controls and playback with a private silent PulseAudio sink
(requires `pulseaudio`). Regenerate the tiny media fixtures with `python3 manage.py media-fixtures`
when needed (requires `ffmpeg`).

Root executables are `worminal`/`worminald` (GTK) and `worminal-x11`/`worminald-x11`
(rollback). Preview names are compatibility builds created on demand.
Build the release and rollback pairs together with `python3 manage.py build-x11`.
Generated modules, objects, test binaries, and compiled configuration headers belong in `build/`;
diagnostic captures belong in `.checks/`.
`python3 manage.py clean` removes build outputs, including legacy root objects, while preserving `.checks/`.
`python3 manage.py check-build` checks build and cleanup behavior without compiling the app.
Keep appearance in the Alacritty configuration described above and terminal defaults in `src/config.h`;
edit neither generated headers nor compiler command records. Application styling belongs to each HTML application.

| Path | Responsibility |
| --- | --- |
| [src/terminal](src/terminal) | Terminal state and scrollback |
| [src/gtk](src/gtk) | Release windows, terminal rendering, and browser tabs |
| [src/x11](src/x11) | Original X11 frontend for rollback |
| [src/workspace](src/workspace) | Service, client connections, and wire protocol |
| [tools](tools) | Build configuration and management helpers |
| [tests](tests) | Regression tests, display proofs, and benchmarks |
| [assets](assets) | Desktop integration and terminfo |
| `build/` | Generated headers, objects, and test binaries |

### Checks and measurements

```sh
python3 manage.py check-handoff   # Live ownership transfer between X11 and GTK daemons
python3 manage.py check-headless  # Native, service, theme, and privacy tests without X11
python3 manage.py check           # Local regression suite, including private-display checks
python3 manage.py lint            # Text limits and Clang static analysis
python3 manage.py latency         # Launch to first echoed input, using /bin/cat
python3 manage.py latency --forwarded  # Another window in an existing GTK owner
python3 manage.py latency-shell   # Launch to input received by your interactive shell
python3 manage.py latency-visible # Keypress to visible echo; fresh and additional windows
python3 manage.py browser-perf    # First/additional browser conversions to visible page pixels
python3 manage.py interaction-perf # Tab ownership, cached switching, and wheel/page pixels
python3 manage.py render-perf      # Release-module painting, exact pixels, CPU, and memory
```

X11 checks and launch benchmarks use a private Xvfb display, keeping test windows and keystrokes off your
desktop. They need `Xvfb`, `xdotool`, and XTest development headers (`libxtst-dev` on Debian).
Pixel benchmarks require python-xlib and Pillow. Latency samples and GTK startup phases are saved
under `.checks/latency-*.json` for comparisons.
`latency` reports median and 95th percentile across 20 runs, excluding Xvfb startup; the `/bin/cat` probe also
excludes shell startup. Windows map before fonts load so X can queue early keystrokes.

`latency-visible --trials 10 --keys 50` measures real XTest key events through the PTY and GTK renderer
until expected glyph pixels are readable from the X server. Unmeasured warm-up keys calibrate the expected
images so resize repaints cannot masquerade as echoes. A raw-mode echo fixture hides the cursor to
avoid blink false positives. Each trial measures both a fresh frontend and another window in its running
owner, reporting launch-to-map and launch-to-visible-fixture readiness separately. Filesystem caches and
the private daemon remain warm after the first trial starts it; this is not a cold-boot benchmark or
interactive-shell startup measurement.
Median, p95, maximum, and raw samples are saved in `.checks/visible-latency.json`. Pixel polling sleeps
1 ms between observations and adds readback overhead. Xvfb excludes physical keyboard latency, desktop
compositing, and monitor scanout; use the same host, theme, and load for comparisons.

`python3 manage.py profile-startup` records startup syscalls with `strace` on a private display;
add `--shell` to include interactive initialization. It reuses the visible-readiness fixture and writes
`.checks/startup-syscalls.log`, startup milestones, and `.checks/startup-profile.json` with the actual
window-owner PID and readiness cutoff in Unix nanoseconds. The trace follows private threads and children
through cleanup, so filter at that cutoff when studying startup. Tracing adds substantial overhead;
use `latency-visible` for latency comparisons.

For launch changes, save a binary/module baseline before editing, then alternate baseline/candidate trials:

```sh
python3 manage.py latency-visible --snapshot .checks/launch-baseline
python3 manage.py latency-visible --compare .checks/launch-baseline --trials 20 --keys 3
```

Each version gets a private display and workspace with the same growing tab workload; trial order alternates.
The native echo fixture excludes interpreter startup and restores its ready canvas after startup resizes.
Use `--saved-tabs 100` to seed a populated workspace before measuring launch costs.
Add `--varied-labels` to use distinct directory basenames; seeding checks each readiness identity
and verifies the exact retained tab count before measurement.
Add `--trace-startup` to diagnose cumulative application milestones from launcher entry;
these include logging overhead and supplement the independent pixel measurements.
Use `--shell` to run the configured POSIX-compatible interactive shell before it execs
the pixel fixture, including its real startup configuration. Each measured launch uses
a unique readiness color so an older retained tab cannot satisfy the readiness check.
`--startup-resources` samples frontend/helper CPU and approximate per-process RSS
high-water counters after fresh readiness. It excludes daemon-owned shells;
separate process peaks need not coincide, and sampling can perturb later timings.
The [Linux status counters](https://man7.org/linux/man-pages/man5/proc_pid_status.5.html)
are approximate; this mode does not measure whole-application peak PSS.
`--bridge-delay-ms 100` uses the local SSH fixture with a controlled startup delay. It exercises the
production workspace bridge; actual SSH authentication, compression and network timing remain outside this probe.
`.checks/visible-latency-comparison.json` records raw pairs, differences, CPU count, and start/end system load.

`interaction-perf` observes known tab and scrollback colors after real key/wheel input on private Xvfb.
Use `--pace-ms 40` for isolated gestures; the default sends each request after its previous visible response.
It also records repaint counts and a fixed 100-event wheel burst independent of response speed.

`render-perf --compare .checks/launch-baseline` loads the actual release frontend module and draws dense ASCII,
dense Unicode with wide cells, mixed Unicode and a clipped stripe through the real GTK terminal widget.
Baselines must have a compatible
internal frontend view ABI. It checks exact RGBA pixels and geometry, alternates variant order, and measures
wall time, process CPU and PSS after untimed warm-up frames. `--backend wayland` uses the private native
compositor; the default uses Xvfb. Results and probe/module hashes are saved in `.checks/render-perf-*.json`.
Use `render-perf --resize` to time two complete viewport imports per resize cycle;
painting happens after the timed interval. It also checks final pixels and retained replica storage.
Use `render-perf --tabs 100` to isolate the real tab strip with distinct directory labels and the last
tab active. This checks exact pixels and measures drawing CPU; use `latency-visible --saved-tabs 100
--varied-labels` to measure the corresponding whole-window launch.
`check-handoff --baseline PATH` adds transfers in both directions with a saved v2 daemon binary.
These measurements isolate rendering; they exclude input/service delivery and compositor/display presentation.

`python3 manage.py frame-perf --compare .checks/launch-baseline` measures ordinary PTY output to a complete
service frame without keyboard-response boosting. Its paired writes exercise the end of the batching interval;
raw results are saved under `.checks/frame-output-latency-*.json`. `check-headless` also checks scheduler
deadline boundaries and verifies that the final paced output reaches a complete frame.
The probe records frame count, daemon CPU time and its counter resolution, and daemon RSS/PSS. Correct deadline
handling can deliver more intermediate frames while retaining the same configured batching-rate limit.

Lint requires `clang-tidy`. It enforces 120-character lines and a 1,000-line file limit, with existing larger
files capped at their recorded lengths until split.

`python3 manage.py proof` runs the regression suite and all private-display proofs, including Qtile, KWin,
and remote Cinnamon. Use `proof plasma` or `proof cinnamon` for one managed proof.
The remote proof requires `WORMINAL_CINNAMON_HOST=user@host` in the environment.
`python3 manage.py proof movement` checks that decorated and borderless windows stay above the screen
after an Alt-drag under private Muffin, comparing GTK and X11 Worminal with an `xmessage` control. It requires
Muffin, `xmessage`, python-xlib, and Rich in addition to the display-test dependencies.
The [shared-tab proof ledger](docs/shared-tabs.md) records earlier X11 proofs.

`python3 manage.py master-perf user@host` measures full-frame SSH tab-switch traffic using an isolated service.
Set `WORMINAL_MASTER_DAEMON` to a remote daemon path to compare builds without touching existing tabs.
After editing [assets/worminal.svg](assets/worminal.svg), run `python3 manage.py icon` to regenerate the embedded
window icon; this needs Inkscape and Pillow.

### Contributing

After cloning, enable the repository's privacy hooks and anonymous commit identity:

```sh
python3 manage.py privacy-setup
python3 manage.py check-privacy
```

Keep machine destinations in environment configuration. Commit hooks check staged content, commit messages,
and identities; push hooks and CI scan reachable history. Use `check-privacy --history` to run that scan locally.
Reports show locations and categories without matched values; pattern checks cannot guarantee complete coverage.
Submodules have separate histories: audit the explorer with
`python3 manage.py check-privacy --repository file-explorer --history` before publishing its commits.
The October 2026 audit of explorer revision `db3c883` found no matches across its four commits.
Its pinned public Hyperlinter dependency retains upstream author identities and fixture email addresses;
its scanned history had no credential, private-key, private-network, or home-path findings.
Publish explorer commits to its public repository before publishing a Worminal commit that references them.
Require the Privacy CI job in branch rules to enforce checks even when hooks are bypassed.
Create merge commits locally with the anonymous identity: server-generated merges can fail the history check.

## Origins and license

Worminal is a fork of st 0.9.3, upstream commit `04ce0d643ed17793803e8516f4c9a5b13b93c400`.
See [LICENSE](LICENSE) for copyright and license terms. The previous Tauri implementation remains in Git history.
