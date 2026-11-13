# Desktop notifications

Worminal accepts the [OSC 99 protocol](https://sw.kovidgoyal.net/kitty/desktop-notifications/)
for title/body desktop notifications. Notifications appear on the controlling view's desktop,
including when a shell runs remotely. Detached terminals have no notification destination.
Delivery uses the session desktop notification service asynchronously. Clicking a notification selects
its originating tab and presents that window. Closed tabs/windows are ignored. The desktop decides
whether focus can be granted; supplied activation tokens are forwarded to GTK.

```sh
printf '\033]99;;Build finished\033\\'
printf '\033]99;i=build:d=0;Build finished\033\\'
printf '\033]99;i=build:p=body;All checks passed\033\\'
```

Supported metadata: `i` (identifier for assembling chunks), `p=title` (default), `p=body`,
`d=0` (more chunks follow), `d=1` (complete, default), and `e=1` (Base64 UTF-8 payload).
Text chunks append within the same tab and identifier. Payload semicolons are preserved.
A body without a title becomes the title. Both BEL and ST terminate commands.
`a=focus` is the default click action; `a=-focus` disables it, including when set on an earlier chunk.
`p=?` queries report title/body payloads, `o=always` delivery and `a=focus`.

Activation reports, application-visible close tracking, replacement of delivered notifications, icons,
buttons, urgency, sounds and expiry overrides are not implemented. Unknown metadata is
ignored and unknown payload types are ignored. Desktop policy determines appearance and lifetime.
Bodies are escaped as plain text for desktop services that interpret markup.

Pending assembly is limited to 32 messages per window and 16 KiB of decoded text per message.
At most 32 desktop deliveries per window may be in flight; further deliveries are dropped until a slot is free.
Excess new pending messages are ignored; an oversized message discards its pending assembly.
Click routing retains at most 128 delivered notifications per window; older notifications lose their
click target when this limit is reached. Closing a notification releases its target, and notification
service restarts clear old targets so reused desktop ids cannot activate unrelated tabs.
Pending notifications are transient and are not saved during recovery or owner transfer.
Older views that do not negotiate notification support receive no notification packets.

`python3 manage.py check-notifications` exercises delivery and click activation on a private X11
desktop with a mock notification service (system Python needs PyGObject). The same test runs in
`check-rust-app`; `check-rust-frontend --unit-only` also checks private D-Bus action/token routing,
close events, service restarts, sender filtering and resource bounds.
