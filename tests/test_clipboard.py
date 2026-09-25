"""Copy a terminal selection to the X11 clipboard through the keyboard shortcut."""

import os
from pathlib import Path
import re
import shutil
import subprocess
import time
import unittest

from x11 import isolated_display


ROOT = Path(__file__).resolve().parent.parent


@unittest.skipUnless(shutil.which("xclip"), "xclip is not installed")
class ClipboardShortcutTest(unittest.TestCase):
    def test_shift_selection_and_ctrl_shift_c_with_mouse_reporting(self):
        with isolated_display() as env:
            title = f"Worminal copy {os.getpid()}"
            terminal = subprocess.Popen(
                [str(ROOT / "worminal"), "-g", "40x10", "-T", title,
                 "-e", "/bin/sh", "-c",
                 "printf '\\033[?25l\\033[?1000hCOPY TEST\\n'; exec cat"],
                env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
            )
            try:
                deadline = time.monotonic() + 5
                window = None
                while time.monotonic() < deadline and not window:
                    result = subprocess.run(
                        ["xdotool", "search", "--onlyvisible", "--name", title],
                        env=env, capture_output=True, text=True)
                    if result.returncode == 0:
                        window = result.stdout.splitlines()[0]
                    else:
                        time.sleep(.05)
                self.assertIsNotNone(window, "terminal window did not open")
                deadline = time.monotonic() + 5
                increments = None
                while time.monotonic() < deadline and increments is None:
                    hints = subprocess.check_output(
                        ["xprop", "-id", window, "WM_NORMAL_HINTS"], env=env, text=True)
                    increments = re.search(r"resize increment:\s*(\d+) by (\d+)", hints)
                    if increments is None:
                        time.sleep(.05)
                self.assertIsNotNone(increments, hints)
                cw, ch = map(int, increments.groups())
                geometry = subprocess.check_output(
                    ["xdotool", "getwindowgeometry", "--shell", window], env=env, text=True)
                origin = dict(line.split("=", 1) for line in geometry.splitlines() if "=" in line)
                x = int(origin["X"]) + 2 + cw // 2
                y = int(origin["Y"]) + 2 + ch + ch // 2
                time.sleep(.2)
                subprocess.run(["xdotool", "windowfocus", "--sync", window], env=env, check=True)

                def copy(start, end, caps=False):
                    subprocess.run(["xdotool", "mousemove", str(x + start * cw), str(y)],
                                   env=env, check=True)
                    subprocess.run(["xdotool", "keydown", "shift", "mousedown", "1",
                                    "mousemove", str(x + end * cw), str(y),
                                    "mouseup", "1", "keyup", "shift"], env=env, check=True)
                    if caps:
                        subprocess.run(["xdotool", "key", "Caps_Lock"], env=env, check=True)
                    subprocess.run(["xdotool", "key", "ctrl+shift+c"], env=env, check=True)
                    return subprocess.check_output(
                        ["xclip", "-selection", "clipboard", "-out"], env=env,
                        timeout=3, text=True)

                self.assertEqual(copy(0, 3), "COPY")
                self.assertEqual(copy(5, 8, caps=True), "TEST")
            finally:
                if terminal.poll() is None:
                    terminal.terminate()
                terminal.communicate(timeout=3)
