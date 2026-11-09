# Worp architecture

[The README](../README.md) defines Worp. Its maintained implementation is the public
`worminal_protocol::worp` module, above the existing workspace framing.

Worp owns **what a node asks another node to do**, independently of delivery:

- `Request`: list tabs, read a terminal viewport, send input, or explicitly enroll/list/forget device routes.
- `Reply`: operation status and the same JSON application document on every transport.
- `Handler`: performs the request against a node's application state.
- `Transport`: delivers a request and returns its reply.
- `Client`: validates requests and talks to a chosen transport.

These initial Worp operations cover the agent-facing API. GUI viewport replication continues to use its existing
v2 messages alongside the Worp binding.

Device membership is owned by the queried authority; see [the device registry](devices.md). Its availability
observations run on demand in the caller, so network waits never block a terminal service.

The daemon implements `Handler`; its terminal operations do not receive sockets, stream packet headers or SSH
processes. The CLI constructs Worp requests and uses `Client`. Changing the communication medium replaces the
transport adapter without changing those requests or terminal operations. `Direct` delivers to a local handler
without serialization, demonstrating that the high-level protocol itself needs no byte stream.

## Delivery adapters

The `stream` adapter carries Worp over an ordered, reliable `Read`/`Write` stream. Its `v2` binding maps Worp
requests and replies to the existing bounded workspace envelope. This keeps the established per-user service
and its GUI protocol intact. Application operations and the stream envelope are separate contracts.

Production entry points currently support:

- **Local Unix socket:** the daemon's existing same-user authentication, with bounded queues and timeouts.
- **SSH:** `worminald api --master user@host COMMAND` uses the existing pinned SSH process helper. SSH starts
  `worminald api --stdio` on the remote node. That endpoint forwards one Worp request to the existing local
  service and sends the reply back. Request text travels in Worp, never in the remote shell command. It does
  not start or restore a workspace. SSH provides authentication and encryption.

The same stream adapter is exercised over real loopback TCP in tests. This establishes transport compatibility,
not a production TCP listener, discovery system or TCP authentication policy. No TCP port is opened by the product.
Raw UDP and HTTP adapters are not implemented. They can implement `Transport` and dispatch decoded requests to
`Handler`; they must supply their own authentication, message boundaries, deadlines and reply correlation.
A datagram adapter must also account for loss, duplication and reordering before it is suitable for input.

Neither Worp nor its adapters automatically retry uncertain input delivery. A successful input reply means queued
for the PTY, not processed by an application. These semantics stay the same across delivery methods.

See [the agent interface](automation.md) for commands and the v2 stream binding.
Run `python3 manage.py check-agent-api` for retained Worp, daemon and private-workspace checks, including identical
in-process/Unix/TCP operation results and SSH-stdio forwarding through an isolated transport fixture.
