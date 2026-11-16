"""Private replacement fixture: resize existing clients when a new client maps."""
from pathlib import Path
import select
import sys
from Xlib import X, display

ready, resized = map(Path, sys.argv[1:3])
original = set(map(int, sys.argv[3:]))
connection = display.Display()
root = connection.screen().root
root.change_attributes(event_mask=X.SubstructureNotifyMask)
connection.sync()
ready.touch()
poller = select.poll()
poller.register(connection.fileno(), select.POLLIN)
while True:
    if not connection.pending_events():
        poller.poll()
    while connection.pending_events():
        event = connection.next_event()
        if event.type == X.MapNotify and event.window.id not in original:
            for identity in original:
                window = connection.create_resource_object('window', identity)
                window.configure(width=333, height=222)
            connection.sync()
            resized.touch()
