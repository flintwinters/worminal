# Automatic window updates

Owners built with window replacement watch the executable, frontend and browser module files. An atomic file
replacement opens the updated owner with the same windows, selected tabs, font zoom and window state, then closes
the old windows after the replacement is ready. On X11 it restores window position and size. On Wayland the
compositor controls placement; size and selected tabs are restored, but coordinates cannot be guaranteed.

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
PYTHONPATH=tests python3 tests/test_window_replacement.py
```

These checks modify copied binaries under `build/checks`, never the installed application or desktop session.
