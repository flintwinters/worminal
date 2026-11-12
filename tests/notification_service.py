"""Private desktop notification fixture; control signals use its authenticated D-Bus sender."""
import json
import sys

from gi.repository import Gio, GLib

NAME = "org.freedesktop.Notifications"
PATH = "/org/freedesktop/Notifications"
XML = """<node><interface name='org.freedesktop.Notifications'><method name='Notify'>
<arg type='s' direction='in'/><arg type='u' direction='in'/><arg type='s' direction='in'/>
<arg type='s' direction='in'/><arg type='s' direction='in'/><arg type='as' direction='in'/>
<arg type='a{sv}' direction='in'/><arg type='i' direction='in'/><arg type='u' direction='out'/>
</method></interface></node>"""


def main():
    bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    bus.call_sync("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "RequestName",
                  GLib.Variant("(su)", (NAME, 0)), None, Gio.DBusCallFlags.NONE, 5000, None)
    identity = 0

    def notify(_bus, _sender, _path, _interface, _method, parameters, invocation):
        nonlocal identity
        identity += 1
        invocation.return_value(GLib.Variant("(u)", (identity,)))
        print(json.dumps({"id": identity, "actions": parameters.unpack()[5]}), flush=True)

    registration = bus.register_object(PATH, Gio.DBusNodeInfo.new_for_xml(XML).interfaces[0], notify, None, None)
    loop = GLib.MainLoop()

    def control(_source, _condition):
        line = sys.stdin.readline()
        if not line:
            loop.quit()
            return False
        command = json.loads(line)
        bus.emit_signal(None, PATH, NAME, "ActionInvoked",
                        GLib.Variant("(us)", (command["id"], "default")))
        bus.flush_sync(None)
        print(json.dumps({"clicked": command["id"]}), flush=True)
        return True

    GLib.io_add_watch(sys.stdin, GLib.IO_IN | GLib.IO_HUP, control)
    print(json.dumps({"ready": True}), flush=True)
    try:
        loop.run()
    finally:
        bus.unregister_object(registration)


if __name__ == "__main__":
    main()
