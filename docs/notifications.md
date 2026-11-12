# Desktop notifications

Worminal accepts the [OSC 99 protocol](https://sw.kovidgoyal.net/kitty/desktop-notifications/)
for title/body desktop notifications. Notifications appear on the controlling view's desktop,
including when a shell runs remotely. Detached terminals have no notification destination.
Delivery uses the session desktop notification service asynchronously.

```sh
printf '\033]99;;Build finished\033\\'
printf '\033]99;i=build:d=0;Build finished\033\\'
printf '\033]99;i=build:p=body;All checks passed\033\\'
```

Supported metadata: `i` (identifier for assembling chunks), `p=title` (default), `p=body`,
`d=0` (more chunks follow), `d=1` (complete, default), and `e=1` (Base64 UTF-8 payload).
Text chunks append within the same tab and identifier. Payload semicolons are preserved.
A body without a title becomes the title. Both BEL and ST terminate commands.
`p=?` queries report the implemented title/body payloads and `o=always` delivery.

Activation actions/reports, close tracking, replacement of delivered notifications, icons,
buttons, urgency, sounds and expiry overrides are not implemented. Unknown metadata is
ignored and unknown payload types are ignored. Desktop policy determines appearance and lifetime.
Bodies are escaped as plain text for desktop services that interpret markup.

Pending assembly is limited to 32 messages per window and 16 KiB of decoded text per message.
At most 32 desktop deliveries may be in flight; further deliveries are dropped until a slot is free.
Excess new pending messages are ignored; an oversized message discards its pending assembly.
Pending notifications are transient and are not saved during recovery or owner transfer.
Older views that do not negotiate notification support receive no notification packets.
