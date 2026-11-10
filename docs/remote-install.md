# Install a new remote node

From an existing master:

```sh
worminald install-on-remote felix@new-disciple
```

The native command installs the **headless daemon and manual** into `/usr/local`. It uses your existing SSH
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

Only the running daemon inode and embedded manual are sent. The master validates target OS/CPU before sending
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

Run `python3 manage.py check-remote-install` for private mock-SSH/bootstrap and no-overwrite publication checks.
It uses neither root nor live remote hosts.
