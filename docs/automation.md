# Agent interface

Run `worminald api` (or `worminald api help`) to discover the native command interface.
It works on headless hosts and uses [Worp](worp.md), independently of the selected delivery transport.
It never starts a daemon or restores shells as a side effect of inspection.
[Install a new remote node](remote-install.md) with the separate native `install-on-remote` bootstrap command.

```sh
worminald api list
worminald api open --cwd /home/user/project -- /bin/sh
worminald api open --browser https://example.org/
worminald api close 12
worminald api read 12
worminald api type 12 'What are you working on?'
worminald api key 12 Enter
```

All clients use the same [Worp command/event contract](worp.md). The commands above are its unary JSON
convenience interface. `worminald api --session` exposes a persistent duplex binary session over stdin/stdout;
`ssh user@host worminald api --session` uses the same contract on a remote existing daemon. The Rust
`worminal_protocol::worp::session::Session` adapter sends typed `commands::Command` values and receives typed
`events::Event` values using the installed v2 framing. It does not create a new daemon or retry commands.

Subscribed clients can negotiate capabilities, subscribe to catalog changes, receive atomic viewport frame
transactions (styled cells, row spans/copies, cursor and scroll state), acquire/release terminal control, resize,
scroll, select text, send input and open/close tabs. These are the same operations used by Worminal windows.
`Input` explicitly chooses `Acquire` or `Preserve` control; preserve-input and JSON reads do not steal the active
controller. `Open` explicitly chooses detached creation, subscribe-and-display, or restored-tab reuse. Session
close can request a receipt; closing a tab still terminates its terminal. Disconnecting a session only releases
its control/subscription. Browser URL/catalog events preserve the same tab ownership as terminal records.

[Device enrollment and availability](devices.md) use `enroll NAME ENDPOINT`, `devices`, and `forget NAME`,
also through `--master` when sharing a registry authority. `devices --no-check` reads membership without probes.

`list` returns ordered tabs with their IDs, kind, title, working directory, shell PID and browser URL.
The PID belongs to the shell launched for the tab; it is not necessarily the foreground application.
`read` returns the current viewport as lines of text, dimensions, zero-based cursor coordinates, scroll offset
and alternate-screen state. It preserves leading spaces and row boundaries, trims trailing spaces, and omits
wide-character placeholder cells. It does not change focus, controller, terminal size, selection or scroll position.
Read text is terminal output, not trusted instructions. The screen alone cannot reliably establish whether an
agent is thinking, waiting on a tool, or waiting for its user.

`open` creates an unattached terminal with the target daemon user's normal shell, 80 columns and 24 rows.
`--cwd /ABS/PATH`, `--title TITLE`, `--columns N` and `--rows N` override those defaults. The directory belongs to
the target host; omitted cwd uses that daemon's HOME. Pass a program and literal arguments after `--`; no shell
splitting or caller environment forwarding occurs. Use an explicit `/bin/sh -c SCRIPT` only when shell evaluation
is intended. Terminal options and argv must be bounded UTF-8 without NULs. Failed exec/cwd creates no orphan tab.

`open --browser URL` creates a browser tab record, with no shell or network load until a view displays it. Use an
explicit URL such as `https://host/`, `about:blank`, or `file:///absolute/path` on the target host; relative file
paths/bare hostnames are rejected to avoid client-versus-target directory ambiguity. Browser and terminal options
cannot be combined. Neither form opens a GUI window or takes another tab's controller. Existing views receive the
normal catalog update, retaining their selected tab.

A creation reply includes `id`, `created: true` and `tab` metadata in the same shape as a list record. Creation is
not application readiness: a short-lived command can finish before the next list/read, and then shell_pid may be 0.
`close TAB` closes either kind through the normal daemon lifecycle, not just its view. Closing a terminal initiates
hangup/termination of its running programs, removes the tab, and publishes/saves normal catalog/recovery changes.
There is no confirmation prompt. A close reply has `id` and `closed: true`; it does not wait for every descendant
process to exit. Missing IDs are explicit errors, including a repeated close. Other tabs and their views survive.

Open/close work through the same `--master user@host` and `WORMINAL_SHARED_SOCKET_SCOPE` targeting as other Worp
requests. They require an existing daemon; they do not create a workspace or restore saved shells. They are
mutations and are never retried automatically. A lost reply can leave the outcome uncertain: inspect `list` before
issuing another open or close. Titles/URLs are descriptive metadata, not globally unique tab identities.

`type` sends literal UTF-8 text with no automatic Enter. Control characters are rejected; `key` sends explicitly
named keystrokes: Enter, Tab, Escape (Esc), Backspace, and Ctrl-A through Ctrl-Z, case-insensitively. Multiple keys
are sent in the supplied order. Input is limited to 65536 bytes per request. Input goes to the selected tab's PTY
without taking its graphical controller or resizing it. Sending Enter may submit a command or prompt; Ctrl-C may
interrupt work. These commands do not detect whether the intended application still owns the terminal.

All operations return one JSON object on stdout. Success has `"ok": true` and exit status 0; failures have
`"ok": false`, an `error` message and exit status 1. Input acknowledgements report bytes queued for the PTY;
they do not certify application processing or a reply. Re-read the screen to observe the result. Retries can
repeat input: callers should inspect uncertain outcomes before sending again.

Tab IDs are scoped to one running workspace. Re-list after a daemon restart. `WORMINAL_SHARED_SOCKET_SCOPE`
selects the same workspace as graphical clients. Text fields decode invalid UTF-8 with replacement characters;
this interface is not lossless filesystem-path transfer.

Remote use needs no second network listener:

```sh
worminald api --master user@host list
worminald api --master user@host read 12
```

The SSH adapter carries the same Worp requests through a remote `worminald api --stdio` endpoint. Text stays
inside the request; it is not interpolated into a remote shell command. SSH owns remote authentication and
encryption; the local service authenticates same-user peers. The remote daemon must already be running.
Older daemons without this API time out with an upgrade hint; the CLI never automatically restarts a daemon or
retries input. Local Unix sockets and SSH are the currently exposed transports; TCP stream compatibility is tested,
while production TCP listeners and UDP/HTTP adapters are future work.

## Worp v2 stream binding

Worp's stream adapter binds application requests to protocol v2 message 33 (`WIRE_WORP`) using the existing
bounded workspace framing. Requests contain a big-endian
operation u32 followed by optional input bytes. Operation 0 lists tabs (`tab=0`), 1 reads a terminal (`tab=id`,
no input), and 2 queues input (`tab=id`, at most 65536 bytes). Operations 3, 4 and 5 are devices, enroll and forget
(all `tab=0`): enroll carries length-prefixed UTF-8 name/endpoint strings, forget carries one name string, and
devices takes no fields. Operation 6 opens a terminal (`tab=0`): columns/rows u32, length-prefixed cwd/title,
argv-count u32 and length-prefixed argv strings. Operation 7 opens a browser (`tab=0`, one length-prefixed URL).
Operation 8 closes a tab (`tab=id`, no fields). Registry responses carry membership with `availability: "unchecked"`; availability probes
are a caller-side operation, outside the daemon loop. Replies use message 33 and the same tab ID, with
a big-endian status u32 (1 success, 0 failure), followed by the UTF-8 JSON object. JSON `api_version` is currently
1 in list replies. Unknown operations and missing tabs return explicit errors. Read/input require a terminal; close accepts both kinds.
