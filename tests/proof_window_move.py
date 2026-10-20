"""Check Alt-dragging above the screen under Muffin on a private Xvfb display."""

from pathlib import Path
import os
import signal
import subprocess
import time
import unittest

from rich.console import Console
from Xlib import display

from proof_cinnamon import wait_for_property
from x11 import isolated_display


ROOT = Path(__file__).resolve().parent.parent
console = Console()


def command(env, *args):
    return subprocess.check_output(args, env=env, text=True, timeout=10).strip()


def geometry(env, window):
    return dict(line.split("=", 1) for line in
                command(env, "xdotool", "getwindowgeometry", "--shell", window).splitlines())


def check_drag(env, undecorated, control=False, frontend="worminal"):
    title = "Offscreen move proof"
    args = (["xmessage", "-geometry", "640x320", "-title", title, "Control window"] if control else
            [str(ROOT / frontend), "-T", title, "-e", "/bin/cat"])
    terminal = subprocess.Popen(args, env=env)
    try:
        window = command(env, "xdotool", "search", "--sync", "--onlyvisible", "--name", f"^{title}$")
        time.sleep(.5)
        connection = display.Display(env["DISPLAY"])
        try:
            client = connection.create_resource_object("window", int(window))
            if undecorated:
                motif = connection.intern_atom("_MOTIF_WM_HINTS")
                client.change_property(motif, motif, 32, [2, 0, 0, 0, 0])
            connection.sync()
        finally:
            connection.close()
        if undecorated:
            time.sleep(.3)
        command(env, "xdotool", "windowmove", "--sync", window, "150", "200")
        command(env, "xdotool", "windowactivate", "--sync", window)
        origin = geometry(env, window)
        x, y = int(origin["X"]) + 100, int(origin["Y"]) + 180
        command(env, "xdotool", "mousemove", "--sync", str(x), str(y), "keydown", "Alt_L", "mousedown", "1")
        try:
            for offset in range(10, int(origin["Y"]) + 101, 10):
                command(env, "xdotool", "mousemove", "--sync", str(x), str(y - offset))
                time.sleep(.01)
            time.sleep(.2)
            during = geometry(env, window)
        finally:
            command(env, "xdotool", "mouseup", "1", "keyup", "Alt_L")
        time.sleep(.5)
        after = geometry(env, window)
        console.print(f"frontend={frontend} control={control} undecorated={undecorated}: "
                      f"before={origin['Y']} during={during['Y']} after={after['Y']}")
        assert int(during["Y"]) < 0, "Drag did not move the window above the screen"
        assert after["Y"] == during["Y"], "Window snapped back after releasing Alt-drag"
    finally:
        terminal.terminate()
        terminal.wait(timeout=5)


class MovementTest(unittest.TestCase):
    """Both terminal and control must retain the position chosen by the user."""

    env = None

    def test_decorated_worminal(self):
        check_drag(self.env, False)

    def test_decorated_control(self):
        check_drag(self.env, False, control=True)

    def test_undecorated_worminal(self):
        check_drag(self.env, True)

    def test_undecorated_control(self):
        check_drag(self.env, True, control=True)

    def test_decorated_x11(self):
        check_drag(self.env, False, frontend="worminal-x11")

    def test_undecorated_x11(self):
        check_drag(self.env, True, frontend="worminal-x11")


def main():
    with isolated_display() as env:
        env = {**env, "LIBGL_ALWAYS_SOFTWARE": "1", "XDG_SESSION_TYPE": "x11"}
        with (ROOT / "build/window-move-proof.log").open("w") as log:
            manager = subprocess.Popen(["dbus-run-session", "--", "muffin", "--replace"],
                                       env=env, stdout=log, stderr=log, start_new_session=True)
            try:
                wait_for_property(env, "_NET_SUPPORTING_WM_CHECK", 20, manager)
                MovementTest.env = env
                result = unittest.TextTestRunner(verbosity=2).run(
                    unittest.defaultTestLoader.loadTestsFromTestCase(MovementTest))
            finally:
                os.killpg(manager.pid, signal.SIGTERM)
                manager.wait(timeout=5)
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    raise SystemExit(main())
