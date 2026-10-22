"""Exercise launch activation with a desktop's focus-stealing prevention enabled."""
from pathlib import Path
import shutil
import subprocess
import time
import unittest

from preview import profile, service
from x11 import isolated_display

ROOT = Path(__file__).resolve().parent.parent


def wait_for(check, description):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(.02)
    raise AssertionError(description)


@unittest.skipUnless(all(shutil.which(tool) for tool in ("muffin", "dbus-run-session", "Xvfb", "xdotool")),
                     "Muffin, D-Bus, Xvfb and xdotool are required")
class WindowActivationTests(unittest.TestCase):
    def test_forwarded_and_shortcut_launches_activate_new_windows(self):
        with isolated_display() as display, profile(display) as env, service(env):
            env["GIO_USE_VFS"] = "local"
            env["GSETTINGS_BACKEND"] = "memory"
            env["GDK_BACKEND"] = "x11"
            env.pop("DBUS_SESSION_BUS_ADDRESS", None)
            with (ROOT / ".checks/window-activation.log").open("wb") as log:
                manager = subprocess.Popen(
                    ["dbus-run-session", "--", "muffin", "--sm-disable", "--x11"],
                    env=env, stdout=log, stderr=log)
                owner = None

                def output(*args):
                    return subprocess.run(args, env=env, capture_output=True, text=True).stdout.strip()

                def window(title):
                    found = output("xdotool", "search", "--onlyvisible", "--name", f"^{title}$")
                    return found.splitlines()[0] if found else None

                try:
                    wait_for(lambda: "window id #" in output("xprop", "-root", "_NET_SUPPORTING_WM_CHECK"),
                             "Muffin did not start; see .checks/window-activation.log")
                    owner = subprocess.Popen([str(ROOT / "worminal"), "-T", "Activation first",
                                              "-e", "/bin/cat"], env=env, stdout=log, stderr=log)
                    wait_for(lambda: window("Activation first"), "First window did not map")
                    first = window("Activation first")
                    subprocess.run(["xdotool", "windowactivate", "--sync", first,
                                    "key", "a"], env=env, check=True)
                    # Give the owner a real input timestamp before forwarding a launch.
                    time.sleep(.1)
                    subprocess.run([str(ROOT / "worminal"), "-T", "Activation second",
                                    "-o", "-", "-e", "/bin/sh", "-c",
                                    "stty raw -echo; printf ACTIVATION_READY; exec cat"],
                                   env=env, stdout=log, stderr=log, check=True, timeout=10)
                    wait_for(lambda: window("Activation second"), "Second window did not map")
                    second = window("Activation second")
                    wait_for(lambda: output("xdotool", "getwindowfocus") == second,
                             "Forwarded launch did not focus its new window")
                    log_path = ROOT / ".checks/window-activation.log"
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
