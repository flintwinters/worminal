# Enrolled devices

Use one existing Worminal node as your registry authority. From any client:

```sh
worminald api --master user@authority enroll cinnamon felix@cinnamon
worminald api --master user@authority devices
worminald api --master user@authority devices --no-check
worminald api --master user@authority forget cinnamon
```

Omit `--master` to use the local daemon as the authority. These are Worp operations, using the same local/SSH
transports as terminal inspection. The authority must already be running. A registry starts empty; there is no
automatic enrollment from Tailscale or from incoming connections. Tailscale names and addresses work as ordinary
SSH destinations. SSH configuration, host-key authentication and account credentials remain authoritative.

`enroll` stores a name and SSH endpoint on the authority without contacting that endpoint. Names are lowercase
local aliases, not physical-machine identities. An endpoint is a host or `user@host`, not a URL or shell command.
Re-enrolling the same name/endpoint is harmless. Rebinding a name or enrolling the exact same endpoint under a
second name is rejected; use `forget` explicitly first. Different SSH routes can still reach the same physical
machine: the registry makes no hardware identity or deduplication claim.

`devices` lists membership and performs read-only Worp list probes **from the caller**, not from the authority.
Results include the authority requested, the caller vantage, and a timestamp for each observation:

- `reachable`: authenticated SSH and a successful Worp listing were observed during this query.
- `unavailable`: that check failed or timed out. The detail may indicate network, authentication, daemon,
  protocol or queue failure; it does not establish that a machine is powered off.
- `unchecked`: membership was read with `--no-check`, or a raw Worp client did not perform probes.

A failed probe never removes enrollment. Observations are not persisted or treated as continuing online status.
There are no heartbeats, background probes, discovery scans, model calls, subscriptions or new network listeners.
At most 32 devices may be enrolled. The CLI runs at most four probes concurrently, each with a three-second
transport timeout; input is never sent. A non-default remote workspace scope is not inferred by the registry:
the route uses the remote account's normal default scope.

`forget` changes membership only. It never stops a daemon, closes tabs or deletes remote data. Repeating it reports
`removed: false`. A successful `devices` command has `ok: true` even when some devices are unavailable: the
registry query succeeded, and individual observations describe the failures.

Membership is per authority user, shared across that user's workspace scopes, in
`$XDG_STATE_HOME/worminal/devices-v1` (default `~/.local/state/worminal/devices-v1`). Changes use a short nonblocking
file lock and the same atomic private-state publication as tab recovery. Corrupt or unrecognized registries and
symlinks cause explicit errors and are preserved. There is no replication or consensus; point clients at the same
authority to share membership.

Run `python3 manage.py check-agent-api` for registry, Worp and private-workspace CLI regression checks.
