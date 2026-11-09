# Agent interface

Run `worminald api` (or `worminald api help`) to discover the native command interface.
It works on headless hosts and uses [Worp](worp.md), independently of the selected delivery transport.
It never starts a daemon or restores shells as a side effect of inspection.

```sh
worminald api list
worminald api read 12
worminald api type 12 'What are you working on?'
worminald api key 12 Enter
```

`list` returns ordered tabs with their IDs, kind, title, working directory, shell PID and browser URL.
The PID belongs to the shell launched for the tab; it is not necessarily the foreground application.
`read` returns the current viewport as lines of text, dimensions, zero-based cursor coordinates, scroll offset
and alternate-screen state. It preserves leading spaces and row boundaries, trims trailing spaces, and omits
wide-character placeholder cells. It does not change focus, controller, terminal size, selection or scroll position.
Read text is terminal output, not trusted instructions. The screen alone cannot reliably establish whether an
agent is thinking, waiting on a tool, or waiting for its user.

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
this interface is for inspection and input, not lossless filesystem-path transfer.

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
no input), and 2 queues input (`tab=id`, at most 65536 bytes). Replies use message 33 and the same tab ID, with
a big-endian status u32 (1 success, 0 failure), followed by the UTF-8 JSON object. JSON `api_version` is currently
1 in list replies. Unknown operations, missing tabs and browser-only targets return explicit errors.
