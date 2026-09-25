"""Hover and open visible shell URLs through the local X11 window."""

import hashlib
import os
from pathlib import Path
import re
import subprocess
import time
import unittest

from x11 import isolated_display


ROOT = Path(__file__).resolve().parent.parent


class UrlClickTest(unittest.TestCase):
    def test_hover_and_ctrl_click_shell_url(self):
        opener = ROOT / ".checks" / "xdg-open"
        opened = ROOT / ".checks" / f"url-opened-{os.getpid()}"
        gate = ROOT / ".checks" / f"url-gate-{os.getpid()}"
        done = ROOT / ".checks" / f"url-done-{os.getpid()}"
        opener.write_text("#!/bin/sh\nprintf '%s' \"$1\" > \"$WORMINAL_URL_OPENED\"\n")
        opener.chmod(0o755)
        opened.unlink(missing_ok=True)
        gate.unlink(missing_ok=True)
        done.unlink(missing_ok=True)
        terminal = None
        try:
            with isolated_display() as env:
                env = {**env, "PATH": f"{ROOT / '.checks'}:{env['PATH']}",
                       "WORMINAL_URL_OPENED": str(opened)}
                terminal = subprocess.Popen(
                    [str(ROOT / "worminal"), "-g", "40x10", "-T", "URL click test",
                     "-e", "/bin/sh", "-c",
                     "printf '\\033[?25lhttps://example.org/from-ssh\\n'; "
                     f"while [ ! -e '{gate}' ]; do sleep .05; done; "
                     "printf '\\033[H\\033[2Kplain text\\n'; "
                     f": > '{done}'; exec cat"],
                    env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                )
                deadline = time.monotonic() + 5
                window = None
                while time.monotonic() < deadline and not window:
                    result = subprocess.run(
                        ["xdotool", "search", "--onlyvisible", "--name", "URL click test"],
                        env=env, capture_output=True, text=True)
                    if result.returncode == 0:
                        window = result.stdout.splitlines()[0]
                    else:
                        time.sleep(.05)
                self.assertIsNotNone(window, "terminal window did not open")
                geometry = subprocess.check_output(
                    ["xdotool", "getwindowgeometry", "--shell", window],
                    env=env, text=True)
                dimensions = dict(line.split("=", 1) for line in geometry.splitlines() if "=" in line)
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
                self.assertGreater(cw, 0)
                self.assertGreater(ch, 0)
                time.sleep(.2)

                def pixels():
                    image = subprocess.check_output(["xwd", "-id", window, "-silent"], env=env)
                    return hashlib.sha256(image).digest()

                def wait_pixels(expected, same):
                    deadline = time.monotonic() + 3
                    while time.monotonic() < deadline:
                        if (pixels() == expected) == same:
                            return
                        time.sleep(.05)
                    self.fail("URL hover underline did not update")

                subprocess.run(["xdotool", "mousemove", "2", "2"], env=env, check=True)
                time.sleep(.2)
                baseline = pixels()
                subprocess.run(["xdotool", "mousemove",
                                str(int(dimensions["X"]) + 2 + 10 * cw + cw // 2),
                                str(int(dimensions["Y"]) + 2 + ch + ch // 2)],
                               env=env, check=True)
                wait_pixels(baseline, False)
                subprocess.run(["xdotool", "mousemove", "2", "2"], env=env, check=True)
                wait_pixels(baseline, True)
                subprocess.run(["xdotool", "mousemove",
                                str(int(dimensions["X"]) + 2 + 10 * cw + cw // 2),
                                str(int(dimensions["Y"]) + 2 + ch + ch // 2)],
                               env=env, check=True)
                subprocess.run(["xdotool", "keydown", "ctrl", "click", "1", "keyup", "ctrl"],
                               env=env, check=True)
                deadline = time.monotonic() + 3
                while time.monotonic() < deadline and not opened.exists():
                    time.sleep(.05)
                self.assertTrue(opened.exists(), f"URL did not open; hints={hints}")
                self.assertEqual(opened.read_text(), "https://example.org/from-ssh")
                hovered = pixels()
                gate.touch()
                deadline = time.monotonic() + 3
                while time.monotonic() < deadline and not done.exists():
                    time.sleep(.05)
                self.assertTrue(done.exists(), "shell did not replace the URL")
                wait_pixels(hovered, False)
                time.sleep(.2)
                replaced = pixels()
                subprocess.run(["xdotool", "mousemove", "2", "2"], env=env, check=True)
                time.sleep(.2)
                self.assertEqual(pixels(), replaced, "old URL remained underlined after replacement")
        finally:
            if terminal is not None:
                if terminal.poll() is None:
                    terminal.terminate()
                terminal.communicate(timeout=3)
            opener.unlink(missing_ok=True)
            opened.unlink(missing_ok=True)
            gate.unlink(missing_ok=True)
            done.unlink(missing_ok=True)
