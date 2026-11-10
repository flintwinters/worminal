# Active terminal directories

The GTK frontend publishes a read-only session D-Bus interface on Wayland and X11.
Window titles are independent of this interface. No shell integration is required.

Each frontend process owns `org.worminal.Instance.p<PID>`. Discover these names with:

```sh
busctl --user --no-pager list | rg '^org\.worminal\.Instance\.'
```

Call `org.worminal.Windows1.GetWindows` at `/org/worminal/Windows`, using a discovered name
in place of the example below:

```sh
name=org.worminal.Instance.p12345
busctl --user --json=short call "$name" \
  /org/worminal/Windows org.worminal.Windows1 GetWindows
```

The result has D-Bus type `a(ubbays)`: an array of window records. Each record contains:

| Field | Type | Meaning |
| --- | --- | --- |
| Window ID | `u` | Stable within this frontend process; not an X11 window ID. |
| Focused | `b` | Whether the window currently has desktop focus. |
| Available | `b` | Whether its selected tab has a known terminal working directory. |
| Working directory | `ay` | Full absolute filesystem path as bytes, with no trailing NUL. |
| Master | `s` | Active tab's SSH host (`--master` joins the active window); empty for local tabs. |

Byte arrays preserve spaces, Unicode, and non-UTF-8 filenames exactly. An unavailable
directory is an empty array with `Available=false`: this includes browser tabs, initial
connection, daemon disconnection, and unreadable process directories. An unfocused window
can have an available directory. If no window is focused, there is no globally active
Worminal terminal; do not substitute the last focused window. Remote paths belong to the
remote host identified by `Master`.

This Python example reads the focused terminal directory from every running instance:

```python
import json
import os
import subprocess

listing = subprocess.check_output(
    ["busctl", "--user", "--no-legend", "--no-pager", "list"], text=True)
for line in listing.splitlines():
    name = line.split()[0]
    if not name.startswith("org.worminal.Instance."):
        continue
    try:
        result = subprocess.check_output([
            "busctl", "--user", "--json=short", "call", name,
            "/org/worminal/Windows", "org.worminal.Windows1", "GetWindows"], text=True)
    except subprocess.CalledProcessError:  # An instance can exit during discovery.
        continue
    for window, focused, available, directory, master in json.loads(result)["data"][0]:
        if focused:
            print(name, window, master or "local",
                  os.fsdecode(bytes(directory)) if available else "unavailable")
```

Subscribe to `org.worminal.Windows1.Changed` at the same object path for updates.
It carries the complete current array using the same type as `GetWindows`.
Subscribe before reading the initial snapshot to avoid missing an intervening change.
`org.freedesktop.DBus.NameOwnerChanged` reports instance startup and exit; discard its
windows when the bus name disappears. Closing one window removes its record.

```sh
gdbus monitor --session --dest "$name" \
  --object-path /org/worminal/Windows
```

Selection and focus changes update immediately in the GTK event loop. Directory changes
are detected after PTY output and by a 250 ms daemon check, including silent changes.
The directory is that of the launched shell/process, not an arbitrary foreground descendant.
The interface requires a session bus; without one, the terminal still operates normally.
The legacy `worminal-x11` frontend does not publish this interface.

`src/frontend/src/window_state_protocol.rs` is the authoritative wire contract.
Run `python3 manage.py check-window-state` for the retained frontend, or `check-rust-app` for the Rust app;
both exercise the same private-bus/private-display integration checks, including non-UTF-8 paths.
`manage.py setup-window-activation` prepares a project-local Muffin runtime for the separate
activation checks; it does not install a window manager or change the desktop session.
