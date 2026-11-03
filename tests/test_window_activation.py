"""Exercise launch activation with a desktop's focus-stealing prevention enabled."""
from runtime import CHECKS

from runtime import runtime_binary

from pathlib import Path
import os
import shutil
import subprocess
import sys
import time
import unittest

from preview import profile, service
from x11 import isolated_display

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
from tools import muffin

FRONTEND = runtime_binary(os.environ.get("WORMINAL_FRONTEND", "worminal"))


def wait_for(check, description):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(.02)
    raise AssertionError(description)


@unittest.skipUnless(muffin.available() and all(shutil.which(tool) for tool in ("dbus-run-session", "Xvfb", "xdotool")),
                     "Muffin, D-Bus, Xvfb and xdotool are required")
class WindowActivationTests(unittest.TestCase):
    def test_forwarded_and_shortcut_launches_activate_new_windows(self):
        with isolated_display() as display, profile(display) as env, service(env):
            env["GIO_USE_VFS"] = "local"
            env["GSETTINGS_BACKEND"] = "memory"
            env["GDK_BACKEND"] = "x11"
            env.pop("DBUS_SESSION_BUS_ADDRESS", None)
            with (CHECKS / "window-activation.log").open("wb") as log:
                manager = subprocess.Popen(
                    ["dbus-run-session", "--", *muffin.command(env)],
                    env=env, stdout=log, stderr=log)
                owner = None

                def output(*args):
                    return subprocess.run(args, env=env, capture_output=True, text=True).stdout.strip()

                def window(title):
                    found = output("xdotool", "search", "--onlyvisible", "--name", f"^{title}$")
                    return found.splitlines()[0] if found else None

                def user_time(identity):
                    from Xlib import X, display as xdisplay
                    connection = xdisplay.Display(env["DISPLAY"])
                    try:
                        window = connection.create_resource_object("window", int(identity))
                        separate = window.get_full_property(connection.intern_atom("_NET_WM_USER_TIME_WINDOW"),
                                                            X.AnyPropertyType)
                        if separate is not None:
                            window = connection.create_resource_object("window", int(separate.value[0]))
                        value = window.get_full_property(connection.intern_atom("_NET_WM_USER_TIME"),
                                                         X.AnyPropertyType)
                        return int(value.value[0]) if value is not None else 0
                    finally:
                        connection.close()

                try:
                    wait_for(lambda: "window id #" in output("xprop", "-root", "_NET_SUPPORTING_WM_CHECK"),
                             "Muffin did not start; see build/checks/window-activation.log")
                    owner = subprocess.Popen([str(FRONTEND), "-c", "WorminalActivation", "-n", "activation-fixture",
                                              "-T", "Activation first",
                                              "-e", "/bin/cat"], env=env, stdout=log, stderr=log)
                    wait_for(lambda: window("Activation first"), "First window did not map")
                    first = window("Activation first")
                    self.assertIn("WorminalActivation", output("xprop", "-id", first, "WM_CLASS"))
                    subprocess.run(["xdotool", "windowactivate", "--sync", first,
                                    "key", "a"], env=env, check=True)
                    # Give the owner a real input timestamp before forwarding a launch.
                    time.sleep(.1)
                    previous_time = user_time(first)
                    subprocess.run([str(FRONTEND), "-T", "Activation second",
                                    "-o", "-", "-e", "/bin/sh", "-c",
                                    "stty raw -echo; printf ACTIVATION_READY; exec cat"],
                                   env=env, stdout=log, stderr=log, check=True, timeout=10)
                    wait_for(lambda: window("Activation second"), "Second window did not map")
                    second = window("Activation second")
                    # Some Muffin versions permit zero/stale activation anyway. Independently require
                    # the fresh map timestamp that stricter desktop focus policies need; wrap at 32 bits.
                    wait_for(lambda: 0 < (user_time(second) - previous_time) % (1 << 32) < (1 << 31),
                             "Explicit launch did not refresh its server timestamp")
                    wait_for(lambda: output("xdotool", "getwindowfocus") == second,
                             "Forwarded launch did not focus its new window")
                    log_path = CHECKS / "window-activation.log"
                    wait_for(lambda: b"ACTIVATION_READY" in log_path.read_bytes(), "Raw terminal readiness")
                    before = output("xdotool", "search", "--onlyvisible", "--pid", str(owner.pid))
                    subprocess.run(["xdotool", "key", "ctrl+n", "type", "PASSTHROUGH"], env=env, check=True)
                    wait_for(lambda: b"\x0ePASSTHROUGH" in log_path.read_bytes(),
                             "Ctrl+N did not reach the terminal as byte 0x0e")
                    self.assertEqual(output("xdotool", "search", "--onlyvisible", "--pid", str(owner.pid)),
                                     before, "Ctrl+N opened a window")
                    subprocess.run(["xdotool", "key", "ctrl+shift+n"], env=env, check=True)
                    wait_for(lambda: output("xdotool", "getwindowfocus") not in (first, second, ""),
                             "New-window shortcut did not focus its window")
                    focused = output("xdotool", "getwindowfocus")
                    self.assertIn("Worminal", output("xdotool", "getwindowname", focused))
                finally:
                    if owner is not None:
                        owner.terminate()
                        owner.wait(timeout=5)
                    manager.terminate()
                    manager.wait(timeout=5)
