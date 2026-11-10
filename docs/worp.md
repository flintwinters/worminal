# Worp architecture

[The README](../README.md) defines Worp. Its maintained implementation is the public
`worminal_protocol::worp` module, above the existing workspace framing.

Worp defines commands and events for every client, independently of delivery. `Command` covers tab
lifecycle, terminal input, focus, resizing, scrolling, selection, browser operations and subscription.
`Event` carries catalog updates, viewport frames and cell data, and other state changes. Any client can
use these operations; the protocol does not classify clients as windows or agents.

Open and input commands express their policies explicitly: `Subscribe`, `Detached` and `ReuseRestored`
control opening; `Acquire` and `Preserve` control input ownership. A `Request` and `Reply` provide a
one-shot JSON convenience for inspection, tab operations and device enrollment. The same commands and
events are available through `session::Session`, an ordered `Read`/`Write` adapter. Callers own
subscription, authentication and deadlines. A successful input receipt means bytes were queued, not
that an application processed them; uncertain mutations are never retried automatically.

The daemon owns the state; transports deliver the same contract without deciding the client's purpose.
Device membership is owned by the queried authority; see [the device registry](devices.md). Availability
observations run on demand in the caller, so network waits never block a terminal service.

## Delivery adapters

The session and one-shot adapters carry Worp over ordered, reliable `Read`/`Write` streams. The v2
binding maps commands and events to the existing bounded envelope and preserves deployed message IDs.
Existing v2 peers remain compatible; the full detached launch uses a new operation (9). Application
semantics and stream framing are separate contracts.

A persistent endpoint is `worminald api --session`: stdin receives v2-bound Worp commands and stdout emits
Worp events. It bridges to an existing daemon without starting or restoring one. Over SSH use
`ssh user@host worminald api --session`; the stream contents and semantic contract are identical.
Normal terminal windows send `Command` and consume `Event` through this same binding. The private local
GUI-owner upgrade/descriptor handoff channel is separate internal IPC and is not a node operation.

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

Neither Worp nor its adapters automatically retry uncertain mutation delivery (open, close or input). A successful input reply means queued
for the PTY, not processed by an application. These semantics stay the same across delivery methods.

See [the agent interface](automation.md) for commands and the v2 stream binding.
Run `python3 manage.py check-agent-api` for retained Worp, daemon and private-workspace checks, including identical
in-process/Unix/TCP operation results and SSH-stdio forwarding through an isolated transport fixture.
