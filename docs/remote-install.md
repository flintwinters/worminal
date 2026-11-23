# Install a new remote node

From an existing master:

```sh
worminald install-on-remote felix@new-disciple
```

The native command installs the **daemon and manual** into `/usr/local`. When the target has GTK 3, WebKitGTK 4.1 and Pango/Cairo
and this master is itself a desktop installation, it also installs the `worminal` command, its frontend and browser
modules, the desktop entry and the icon; the staged launcher must run on the target or the install falls back to
the daemon and manual alone. It uses your existing SSH
configuration and authentication; no credentials, source checkout, configuration, registry or user state are copied.
The target needs Linux on the same CPU architecture as the master, Python 3, and non-interactive sudo (or an
explicit root SSH account). The transferred candidate must also pass an unprivileged runtime check on the target;
a matching CPU alone does not guarantee compatible libc or runtime libraries. Dependencies are not installed.

The installer refuses existing Worminal commands, destination files or active workspaces. It is for new nodes,
not an implicit upgrade command. Existing sessions and saved state are never stopped, migrated or rewritten.
The daemon is not started, so installation cannot silently restore saved shells. Open a view explicitly afterward:

```sh
worminal --master felix@new-disciple
```

To enroll that route in a shared registry, use the normal explicit operation:

```sh
worminald api --master user@authority enroll new-disciple felix@new-disciple
```

This bootstrap uses SSH because a new host has no Worp service yet. Once started, its normal agent interface is
Worp. Run `worminald install-on-remote --help`; general daemon help and API help also point to this command.

## Publication and failures

Only the running daemon inode, embedded manual and (for a graphical target) the master's launcher, modules, desktop entry and icon are sent. The master validates target OS/CPU before sending
payload bytes; the target checks existing state and sudo before accepting the transfer. It stages and executes the
candidate as the SSH user before privileged publication. SHA-256 checks keep the published files identical to the
staged payload. Files are published atomically without replacement; a concurrent destination is preserved and the
installer removes only its own partially published files. Privileged writes use directory descriptors and refuse
symlinked or other-user-writable destination directories. The file layout and modes are shared with the normal
headless installation in `assets/headless-install.json`.

Success prints a JSON receipt with `installed: true`, `started: false`, hashes, and the next product command.
Failure prints `ok: false` and a diagnostic. Lost SSH replies can make the outcome uncertain; there is no automatic
retry. Inspect the target before retrying—an existing installation will be refused.
Use [`python3 manage.py upgrade HOST`](remote-upgrade.md) for nodes that already have Worminal.

## Keeping nodes in step

```sh
worminald upgrade-remote felix@disciple
```

Every view connection to a `--master`, including reconnects and replacement owners, checks versions immediately
after SSH authentication. The update finishes before configuration sync, bridge startup, workspace restoration,
subscription or tab creation. These phases share one authenticated SSH connection; a failed update leaves the
view disconnected and never opens a workspace. The GUI remains responsive and background retries never prompt.
It is symmetric:
the node with the older Cargo version is updated from the newer one. A newer local daemon is pushed over the same
bootstrap as `install-on-remote`; a newer remote daemon is pulled through that bootstrap run locally. A daemon that
predates `--version` counts as older. Equal versions change nothing.

A portable daemon is a static musl executable, so it runs on any Linux of the same CPU regardless of either side's
glibc. Default native builds (see development.md) and older daemons are dynamically linked and only run on
peers whose glibc is at least as new as their build host's; the run check refuses them ("cannot run here") and nothing
is installed. A failed sync keeps the local daemon's master link closed: the
connect helper reports the error to the daemon, which sends it to every window (shown in the window and once as a
desktop notification, independent of `update_notifications`) while it retries. The daemon log records the receipt.

The daemon and manual in `/usr/local` are replaced; on a graphics-capable target the sender's `worminal` command,
modules, desktop entry and icon are installed or replaced too, unless the staged launcher cannot load them there,
in which case only the daemon and manual are updated. Running windows hand off to the new files as after a local
install. Replacement is a same-directory
atomic rename after the same staging, checksum, run check and non-interactive sudo steps as installation, and both
sides must be Linux on the same CPU. The running daemon sees its executable change and hands its shells to the new
file without restarting them; connected views reconnect. A daemon too old to watch its executable keeps running
until its next restart. An active v1 workspace, a missing installation or failed sudo leaves everything unchanged.
Outcomes other than "already current" are logged to syslog.

Run `python3 manage.py check-remote-install` for private mock-SSH/bootstrap and no-overwrite publication checks.
It uses neither root nor live remote hosts.

## Master connection history

```sh
worminald --list-masters
worminald --list-masters --help
```

The command returns JSON, for example:

```json
{"ok":true,"masters":[{"endpoint":"felix@disciple","connected":true},{"endpoint":"old-node","connected":false}]}
```

Each row is an exact SSH route that a view successfully connected to. `connected` means at least one connection
has received a valid master catalog and remains open; authentication or version sync alone is not enough.
Multiple windows and aliases remain distinct where appropriate: one row per exact route, live until its last
connection closes. Disconnecting, detaching, crashes and restarts retain the row with `connected: false`.

History begins with this feature; earlier connections are not reconstructed from enrollment or config-sync files.
Reconnect existing views to begin tracking them. History belongs to the local user and spans displays and workspace
scopes sharing `$XDG_STATE_HOME` (default `~/.local/state`). It is stored under `worminal/masters` there; private
connection leases release automatically on process exit. Recording failures go to syslog without breaking a view.
Listing never starts a daemon, opens SSH, probes a host, enrolls devices or takes terminal control. Missing history
returns an empty list without creating files; unreadable or malformed records return `ok: false` and exit status 1.
