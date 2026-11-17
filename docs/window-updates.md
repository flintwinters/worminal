# Automatic window updates

Owners built with window replacement watch the executable, frontend and browser module files. An atomic file
replacement opens the updated owner with the same windows, selected tabs, font zoom and window state, then closes
the old windows after the replacement is ready. On X11 it restores window position and size. On Wayland the
compositor controls placement; size and selected tabs are restored, but coordinates cannot be guaranteed.

Preparation is provisional: windows remain usable while the candidate warms up. Window creation, closure,
tab selection, zoom, geometry and master-link changes during this phase do not invalidate the transaction.
At the commit boundary the donor pauses launch acceptance, gates edits, drains accepted input, and suspends
its viewport readers. Ordered service replies on each existing link fence accepted input through remote
backpressure before that suspension. A reconnected link reissues only its outstanding fence after the new
catalog; terminal input is never replayed. Ordered fence identities prevent a late reply from satisfying a
newer snapshot request. It then sends a fresh descriptor-backed snapshot. The candidate reconciles
to that snapshot, reuses unchanged warm connections, and fences fresh catalogs before profile transfer.

The final presentation update carries late geometry, focus and close requests by stable window identity.
The donor retires only after the candidate accepts the final state and acknowledges commit; accepted shell
input is never replayed and shell ownership remains in the daemon. A close requested during the commit gate
is applied by the new owner, or by the retained donor if preparation fails. New launch connections remain
queued on the continuously bound broker socket during cutover.

Windows containing multiple daemon hosts transfer each link and retain their selected local or remote tab.
The bounded handoff supports 16 additional hosts per window and 128 descriptors across all windows; exceeding
these limits postpones replacement while existing windows and sessions remain available.

Terminal processes remain in the daemon. The replacement attaches to existing tabs and never replays shell
commands. Remote windows require a daemon supporting `--bridge-existing`; an older or missing remote service
postpones the update rather than restoring shells. The first upgrade from an owner predating this feature still
requires closing that old owner once.

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
python3 manage.py check-window-updates
```

Use `--match PATTERN` to select a regression. These checks modify copied binaries on private displays under
`build/checks`, never the installed application or desktop session. They include deterministic edits during warm-up,
a browser first opened during preparation, close/geometry edits and queued launches during profile commit,
accepted input behind remote backpressure, and failed-candidate rollback.
GUI transaction phases and failures are recorded in syslog without window content or cookie payloads.
Timeouts identify the transaction phase. Drain timeouts also record each window/link’s connection, catalog,
pending-tab, queued-byte and fence state, without terminal contents or host credentials.

Update status (postponed, pending or failed) always appears in the window message bar. Desktop notifications for these
notices are off by default; enable them with `update_notifications: true` under `worminal:` in `worminal.yaml` (see `config.md`). One notification is sent per notice, not per window.
