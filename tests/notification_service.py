"""Private-bus stand-in for the desktop notification service: logs each Notify, clicks the latest on SIGUSR1."""
import json
import signal
import sys

from gi.repository import Gio, GLib

NAME = "org.freedesktop.Notifications"
PATH = "/org/freedesktop/Notifications"
XML = f"""<node><interface name='{NAME}'>
  <method name='Notify'>
    <arg type='s' direction='in'/><arg type='u' direction='in'/><arg type='s' direction='in'/>
    <arg type='s' direction='in'/><arg type='s' direction='in'/><arg type='as' direction='in'/>
    <arg type='a{{sv}}' direction='in'/><arg type='i' direction='in'/><arg type='u' direction='out'/>
  </method>
  <signal name='ActionInvoked'><arg type='u'/><arg type='s'/></signal>
</interface></node>"""


def main(log):
    loop = GLib.MainLoop()
    state = {"id": 0, "connection": None}

    def call(connection, _sender, _path, _interface, _method, parameters, invocation):
        application, replaces, icon, summary, body, actions, hints, _timeout = parameters.unpack()
        state["id"] += 1
        with open(log, "a", encoding="utf-8") as output:
            output.write(json.dumps({"id": state["id"], "application": application, "replaces": replaces,
                                     "icon": icon, "summary": summary, "body": body, "actions": actions,
                                     "hints": hints}) + "\n")
        invocation.return_value(GLib.Variant("(u)", (state["id"],)))

    def acquired(connection, _name):
        state["connection"] = connection
        connection.register_object(PATH, Gio.DBusNodeInfo.new_for_xml(XML).interfaces[0], call, None, None)

    def click():
        state["connection"].emit_signal(None, PATH, NAME, "ActionInvoked",
                                        GLib.Variant("(us)", (state["id"], "default")))
        return GLib.SOURCE_CONTINUE

    Gio.bus_own_name(Gio.BusType.SESSION, NAME, Gio.BusNameOwnerFlags.NONE, acquired, None, lambda *_: loop.quit())
    GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGUSR1, click)
    GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGTERM, loop.quit)
    loop.run()


if __name__ == "__main__":
    main(sys.argv[1])
