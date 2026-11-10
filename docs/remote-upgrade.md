# Upgrade an existing device

```sh
python3 manage.py upgrade felix@disciple
```

This internal maintenance command copies **this device's installed `/usr/local` Worminal release** to the existing
remote `/usr/local` installation over SSH. It does not build the checkout, read deleted running binaries, install
packages, enroll devices, or copy credentials, configuration, browser profiles or workspace state. Exact SHA-256
hashes identify the release: two builds can share a package version while containing different code.

`--source-prefix /absolute/prefix` selects another complete installed source. Source and target must both be
headless or both desktop installations. The layout comes from the normal installation rules, including modules,
manual, desktop assets and fonts. Custom `MANPREFIX`/`DESTDIR` layouts outside the source prefix are unsupported.
Use `worminald install-on-remote HOST` for a new device.

The target requires Linux on the same CPU architecture, Python 3, existing SSH authentication and non-interactive
sudo (or root SSH). SSH configuration, aliases, Tailscale routes and keys work normally; password prompts are disabled.
The candidate daemon is run with `api help`; desktop module dependencies and the launcher's version command are
checked without opening windows or starting a daemon. Matching CPU architecture alone does not establish runtime
compatibility. Dependencies are never installed implicitly.

## Sessions and windows

The command publishes files; existing native automatic upgrade mechanisms transfer live daemon sessions and
replace compatible GUI owners. It never sends keystrokes, signals owners, starts a missing daemon or recreates
shells. Closed workspaces remain closed. Browser windows follow [window replacement](window-updates.md): pages
reload, with profile and cookie handoff; volatile page state is lost.

The final JSON receipt separates `installed` from `active`, includes exact hashes and checks the SSH account's
installed owner inodes, frontend/browser module inodes, and existing terminal process identities. A source-identical
installation is a no-op; unchanged files keep their inodes. Verification waits up to 30 seconds for active owners.
When no owners were running, `active: true` means there was nothing to migrate; `started` remains false.

Exit status is zero only when the checked owners use the installed files and observed terminal processes remain.
An installed release can report `active: false` with pending owner PIDs or missing processes, returning status 1.
Legacy owners without automatic replacement remain running; they are never killed to force success. Naturally
exiting processes can also make preservation verification inconclusive. Inspect the receipt before taking further
action. Verification covers the SSH account's owners, not other users' sessions or every website's authentication.

## Publication and recovery

The source is captured consistently before transfer. All bytes are staged privately, hashed and runtime-checked
before publication. The privileged publisher rechecks original hashes, holds an upgrade lock, walks root-owned
non-writable directories without following symlinks, and uses Linux atomic inode exchanges. Modules are published
before launchers. Mid-publication failures restore original inodes when no concurrent publisher has changed them.
Temporary transaction files are removed after success or completed rollback; there is no retained release backup.
User state and live terminal processes are never part of rollback.

There is no multi-file filesystem transaction: an installation interrupted by process/host failure can be partial.
A concurrent change preventing rollback leaves transaction files for inspection. Publication/SSH timeouts or a lost
reply can make the outcome uncertain. The command does not automatically retry, restart shells, or roll back
potentially committed live handoffs. Preserve running sessions, inspect installed hashes and owners, then retry only
when the target state is understood. Root's upgrade lock coordinates this command; other installers must not run
concurrently.

Run the isolated checks with:

```sh
python3 manage.py check-remote-upgrade
```

They use mock SSH/sudo and project-owned prefixes, including a real private daemon with a live shell. They never
contact a live remote device or use root privileges.
