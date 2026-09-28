"""Measure actual X11 tab painting on the current display with an isolated workspace."""

import os
from pathlib import Path
import subprocess
import time
import uuid

from Xlib import X, XK, display
from Xlib.ext import xtest


ROOT = Path(__file__).resolve().parent.parent


def window_for_pid(root, atom, pid):
    for window in root.query_tree().children:
        value = window.get_full_property(atom, X.AnyPropertyType)
        if value and len(value.value) == 1 and value.value[0] == pid:
            return window
        found = window_for_pid(window, atom, pid)
        if found:
            return found
    return None


def pixel(window):
    geometry = window.get_geometry()
    return window.get_image(geometry.width // 2, geometry.height // 2,
                            1, 1, X.ZPixmap, 0xffffffff).data


def wait_for(change, deadline, process):
    while time.monotonic() < deadline:
        result = change()
        if result:
            return result
        if process.poll() is not None:
            raise RuntimeError(f"Worminal exited with status {process.returncode}")
        time.sleep(.005)
    raise TimeoutError("X11 tab did not repaint within 5 seconds")


def keypress(connection, *symbols):
    for symbol in symbols:
        xtest.fake_input(connection, X.KeyPress, connection.keysym_to_keycode(symbol))
    for symbol in reversed(symbols):
        xtest.fake_input(connection, X.KeyRelease, connection.keysym_to_keycode(symbol))
    connection.sync()


def main():
    scope = f"window-perf-{os.getpid()}-{uuid.uuid4().hex}"
    env = dict(os.environ, WORMINAL_SHARED_SOCKET_SCOPE=scope)
    connection = display.Display()
    root = connection.screen().root
    active_atom = connection.intern_atom("_NET_ACTIVE_WINDOW")
    pid_atom = connection.intern_atom("_NET_WM_PID")
    active = root.get_full_property(active_atom, X.AnyPropertyType)
    previous = active.value[0] if active and active.value else None
    command = [str(ROOT / "worminal"), "-g", "120x40", "-T", "window-perf",
               "-e", "/bin/sh", "-c", "printf '\\033[41m\\033[2J'; exec /bin/cat"]
    process = subprocess.Popen(command, env=env, stdout=subprocess.DEVNULL,
                               stderr=subprocess.PIPE)
    try:
        window = wait_for(lambda: window_for_pid(root, pid_atom, process.pid),
                          time.monotonic() + 5, process)
        wait_for(lambda: window.get_attributes().map_state == X.IsViewable,
                 time.monotonic() + 5, process)
        time.sleep(.25)
        initial = pixel(window)
        window.set_input_focus(X.RevertToParent, X.CurrentTime)
        connection.sync()
        keypress(connection, XK.XK_Control_L, XK.XK_t)
        other = wait_for(lambda: pixel(window) if pixel(window) != initial else None,
                         time.monotonic() + 5, process)
        timings = []
        for expected in (initial, other) * 3:
            started = time.monotonic()
            keypress(connection, XK.XK_Control_L, XK.XK_Tab)
            wait_for(lambda: pixel(window) == expected, started + 5, process)
            timings.append(round((time.monotonic() - started) * 1000, 1))
        print(f"Cinnamon X11 tab repaints (ms): {timings}")
    finally:
        if previous:
            connection.create_resource_object("window", previous).set_input_focus(
                X.RevertToParent, X.CurrentTime)
            connection.sync()
        process.terminate()
        try:
            process.communicate(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.communicate()
        subprocess.run([str(ROOT / "worminald"), "--stop"], env=env,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=3)
        connection.close()


if __name__ == "__main__":
    main()
