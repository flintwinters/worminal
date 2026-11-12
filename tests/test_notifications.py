"""Click real OSC 99 notifications on a private desktop and verify the originating tab gains focus."""
from contextlib import ExitStack
import json
import os
from pathlib import Path
import select
import shlex
import signal
import subprocess
import time
import unittest

from runtime import CHECKS, runtime_binary, stop_application, window
from preview import profile, service
from test_keys import read_through
from test_window_activation import wait_for
from test_window_state import private_bus
from x11 import isolated_display

ROOT = Path(__file__).resolve().parent.parent
import sys
sys.path.insert(0, str(ROOT))
from tools import muffin


class NotificationTests(unittest.TestCase):
    def test_click_selects_originating_tab_and_window_and_ignores_closed_origin(self):
        self.assertTrue(muffin.available(), "prepare the private window manager with manage.py setup-check-runtime")
        with (isolated_display() as display, private_bus(display) as session,
              profile(session) as env, ExitStack() as cleanup):
            env.update(GDK_BACKEND="x11", GIO_USE_VFS="local", GSETTINGS_BACKEND="memory")
            env.pop("WAYLAND_DISPLAY", None)
            cleanup.enter_context(service(env))
            log = cleanup.enter_context((CHECKS / "notifications.log").open("wb"))
            manager = subprocess.Popen(muffin.command(env), env=env, stdout=log, stderr=log)
            cleanup.callback(stop_application, manager)

            def output(*args):
                return subprocess.run(args, env=env, capture_output=True, text=True, check=True).stdout.strip()

            wait_for(lambda: "window id #" in output("xprop", "-root", "_NET_SUPPORTING_WM_CHECK"),
                     "private window manager readiness")
            mock = subprocess.Popen(["/usr/bin/python3", str(ROOT / "tests/notification_service.py")],
                                    env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log)
            cleanup.callback(stop_application, mock)

            def response():
                self.assertTrue(select.select([mock.stdout], [], [], 5)[0], "notification fixture response")
                return json.loads(mock.stdout.readline())

            self.assertEqual(response(), {"ready": True})
            notify = "printf '\\033]99;;Build finished\\033\\\\'"
            shell = f"trap {shlex.quote(notify)} USR1; printf 'READY:%s\\n' $$; while :; do sleep .1; done"
            app = subprocess.Popen([str(runtime_binary("worminal-rust")), "-T", "Notification origin", "-o", "-",
                                    "-e", "/bin/sh", "-c", shell], env=env, stdout=subprocess.PIPE, stderr=log)
            cleanup.callback(stop_application, app)
            origin_shell = int(read_through(app.stdout, b"\n").strip().split(b":")[-1])
            origin = window(env, app)
            output("xdotool", "windowactivate", "--sync", origin, "key", "ctrl+shift+t")
            wait_for(lambda: output("xdotool", "getwindowname", origin) == "Worminal", "second tab selection")
            subprocess.run([str(runtime_binary("worminal-rust")), "-T", "Other window", "-e", "/bin/cat"],
                           env=env, stdout=log, stderr=log, check=True, timeout=10)
            wait_for(lambda: output("xdotool", "getwindowfocus") != origin, "other window focus")
            other = output("xdotool", "getwindowfocus")
            os.kill(origin_shell, signal.SIGUSR1)
            notification = response()
            self.assertEqual(notification["actions"], ["default", "Open Worminal"])
            self.assertEqual(output("xdotool", "getwindowfocus"), other, "delivery must not steal focus")

            def click(identity):
                mock.stdin.write((json.dumps({"id": identity}) + "\n").encode())
                mock.stdin.flush()
                self.assertEqual(response(), {"clicked": identity})

            click(notification["id"])
            wait_for(lambda: output("xdotool", "getwindowfocus") == origin, "notification window activation")
            self.assertEqual(output("xdotool", "getwindowname", origin), "Notification origin")
            # Keep a second notification alive, then close its origin. A late click must not
            # select a replacement tab or raise the old window.
            os.kill(origin_shell, signal.SIGUSR1)
            late = response()
            output("xdotool", "key", "ctrl+shift+w")
            wait_for(lambda: output("xdotool", "getwindowname", origin) == "Worminal", "origin tab closed")
            output("xdotool", "windowactivate", "--sync", other)
            click(late["id"])
            deadline = time.monotonic() + .3
            while time.monotonic() < deadline:
                self.assertIsNone(app.poll())
                self.assertEqual(output("xdotool", "getwindowfocus"), other)
                time.sleep(.02)


if __name__ == "__main__":
    unittest.main()
