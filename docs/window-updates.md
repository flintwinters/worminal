# Automatic window updates

Owners built with window replacement watch the executable, frontend and browser module files. An atomic file
replacement opens the updated owner with the same windows, selected tabs, splits, font zoom and window state, then
closes the old windows after the replacement is ready. On X11 it restores window position, size and virtual desktop;
owners before 1.0.4 send neither splits nor desktops, and 1.0.4 sends splits without divider positions. On Wayland the
compositor controls placement; size and selected tabs are restored, but coordinates cannot be guaranteed.

Preparation is provisional: windows remain usable while the candidate warms up. Window creation, closure,
tab selection, zoom, geometry and master attachments during this phase do not invalidate the transaction.
At the commit boundary the donor pauses launch acceptance, gates edits, drains accepted input, and suspends
its viewport readers. An ordered service reply on its daemon link fences accepted input before that suspension;
input bound for a master is then the local daemon's, which outlives the window. A reconnected link reissues only its outstanding fence after the new
catalog; terminal input is never replayed. Ordered fence identities prevent a late reply from satisfying a
newer snapshot request. It then sends a fresh descriptor-backed snapshot. The candidate reconciles
to that snapshot, reuses unchanged warm connections, and fences fresh catalogs before profile transfer.

The final presentation update carries late geometry, focus and close requests by stable window identity.
The donor retires only after the candidate accepts the final state and acknowledges commit; accepted shell
input is never replayed and shell ownership remains in the daemon. A close requested during the commit gate
is applied by the new owner, or by the retained donor if preparation fails. New launch connections remain
queued on the continuously bound broker socket during cutover.

Progress appears in the existing windows. After commit, the replacement windows show an update completion
message for five seconds. With `worminal.update_notifications` enabled, the replacement also requests a
five-second desktop notification. The desktop controls its actual display time.

Each window transfers its one daemon link and retains its selected local or master tab. Master links belong
to the local daemon, so no master, reachable or not, takes part in replacement. Windows of earlier versions
carried their own master links; the receiver hands those masters to the daemon without prompting, and their
master tabs leave the transferred split. The bounded handoff supports 128 descriptors across all windows;
exceeding it postpones replacement while existing windows and sessions remain available.

A peer that never answers SSH reports `Cannot reach HOST` only in the daemon log while it retries; a genuine
version-sync failure is sent to the windows and raises one desktop notification.

## Updates must stay possible

The donor of the next update is the binary running now, and its handoff code cannot be replaced except by the
handoff itself. A donor bug can therefore make updates impossible until its owner is restarted (an unreachable
master did exactly this). Rules that keep every release updatable:

- Receivers read every layout any release has sent. Snapshot versions grow additively
  (`HANDOFF_UPGRADE_MIN_VERSION..=HANDOFF_UPGRADE_VERSION`, `gui_handoff` `1..=VERSION`); never raise the minimum.
- Wire message kinds and capability strings are frozen once shipped; new behavior gets a new number or capability.
  `shipped_wire_vocabulary_is_frozen` and `shipped_snapshot_identity_is_frozen` fail if either changes.
- A donor wait may depend only on state a lost peer cannot hold hostage. Every readiness, fence and drain check
  goes through `Link::quiet()` (settled or lost), donor-side via `links_quiet()`; any new wait needs the same treatment and a deadline.

Terminal processes remain in the daemon. The replacement attaches to existing tabs and never replays shell
commands. Remote windows require a daemon supporting `--bridge-existing`; an older or missing remote service
postpones the update rather than restoring shells. The first upgrade from an owner predating this feature still
requires closing that old owner once.

Connections reconnecting, queued input and pending window launches defer staging; the owner retries the same
installed build once they settle. Replacement notices are recorded in syslog as well as shown in the window.
Before activation the donor validates its complete state. After the candidate maps, geometry may change as a
tiling window manager rearranges clients; commit still validates the installation, tabs, links and other state.

Browser pages reload. Volatile DOM, unsaved form contents, JavaScript memory, back/forward history and active media
are lost. Tabs, URLs and the existing persistent browser profile remain. Cookies are transferred over the private,
same-user update socket in memory, including session cookies and HttpOnly/SameSite attributes; they are never logged
or written to an extra handoff file. A private-X11 HTTP fixture verifies session cookies, persistent cookies and
localStorage across replacement. Website authentication may depend on other volatile state, so retaining cookies
cannot guarantee every site's login. Browser sessionStorage is not transferred.

The old profile owner releases browser views and contexts before the candidate opens the profile. The profile lock
lease remains held through the handoff, and the coordinator waits for pinned WebKit descendants to exit. Unsupported
cookie APIs, a failed candidate or a changed installation cancel the update. Existing terminal sessions remain;
browser pages may reload during rollback. An unresponsive browser worker can delay replacement while the profile
is protected. Candidates never navigate restored browser URLs until commit.

Run the targeted private-display checks after building the native application:

```sh
./manage.py check-window-updates
```

Use `--match PATTERN` to select a regression. These checks modify copied binaries on private displays under
`build/checks`, never the installed application or desktop session. They include deterministic edits during warm-up,
a browser first opened during preparation, close/geometry edits and queued launches during profile commit,
accepted input behind remote backpressure, split browsers, virtual desktops under a private xfwm4 (skipped when
xfwm4 is not installed), and failed-candidate rollback.
GUI transaction phases and failures are recorded in syslog without window content or cookie payloads.
Timeouts identify the transaction phase. Drain timeouts also record each window/link’s connection, catalog,
pending-tab, queued-byte and fence state, without terminal contents or host credentials.

Update status (postponed, pending or failed) always appears in the window message bar. Desktop notifications for these
notices are off by default; enable them with `update_notifications: true` under `worminal:` in `worminal.yaml` (see `config.md`). One notification is sent per notice, not per window.
