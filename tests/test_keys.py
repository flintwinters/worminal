"""Check modified navigation keys through X11, the PTY, and a raw-mode child."""

import os
from pathlib import Path
import select
import shutil
import subprocess
import time
import unittest

from x11 import isolated_display


ROOT = Path(__file__).resolve().parent.parent


def read_bytes(stream, count):
    data = b""
    deadline = time.monotonic() + 3
    while len(data) < count and time.monotonic() < deadline:
        ready, _, _ = select.select([stream], [], [], max(0, deadline - time.monotonic()))
        if ready:
            chunk = os.read(stream.fileno(), count - len(data))
            if not chunk:
                break
            data += chunk
    return data


class NavigationKeysTest(unittest.TestCase):
    @unittest.skipUnless(shutil.which("micro"), "micro is not installed")
    def test_ctrl_end_in_micro(self):
        path = ROOT / ".checks" / "micro-key-test.txt"
        log = ROOT / ".checks" / "micro-output"
        config = ROOT / ".checks" / f"micro-config-{os.getpid()}"
        config.mkdir()
        path.write_text("first\nlast")
        title = f"Worminal micro keys {os.getpid()}"
        with isolated_display() as env:
            terminal = subprocess.Popen(
                [str(ROOT / "worminal"), "-T", title, "-o", str(log), "-e", "micro",
                 "-config-dir", str(config), str(path)],
                env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
            )
            try:
                deadline = time.monotonic() + 5
                window = None
                while time.monotonic() < deadline:
                    result = subprocess.run(
                        ["xdotool", "search", "--onlyvisible", "--class", "Worminal"],
                        env=env, capture_output=True, text=True,
                    )
                    if result.returncode == 0:
                        window = result.stdout.splitlines()[0]
                        break
                    time.sleep(0.05)
                self.assertIsNotNone(window, "micro window did not open")
                subprocess.run(["xdotool", "windowfocus", "--sync", window],
                               env=env, check=True)
                time.sleep(1)
                for keys, letter in (("End", "X"), ("Home", "Y"),
                                     ("ctrl+End", "Z"), ("ctrl+Home", "Q")):
                    subprocess.run(["xdotool", "key", "--clearmodifiers", keys],
                                   env=env, check=True)
                    subprocess.run(["xdotool", "type", "--clearmodifiers", letter],
                                   env=env, check=True)
                subprocess.run(["xdotool", "key", "--clearmodifiers", "ctrl+s", "ctrl+q"],
                               env=env, check=True)
                try:
                    terminal.wait(timeout=5)
                except subprocess.TimeoutExpired as error:
                    raise AssertionError(f"micro did not exit; file contains {path.read_text()!r}; output {log.read_bytes()[:300]!r}") from error
                self.assertEqual(path.read_text(), "QYfirstX\nlastZ\n")
            finally:
                if terminal.poll() is None:
                    terminal.terminate()
                terminal.communicate(timeout=2)
                path.unlink(missing_ok=True)
                log.unlink(missing_ok=True)
                shutil.rmtree(config)

    def test_modified_home_end(self):
        cases = (
            ("ctrl+End", b"\033[1;5F"),
            ("ctrl+Home", b"\033[1;5H"),
            ("shift+End", b"\033[1;2F"),
            ("shift+Home", b"\033[1;2H"),
            ("ctrl+shift+End", b"\033[1;6F"),
        )
        with isolated_display() as env:
            for appcursor in (False, True):
                title = f"Worminal keys {os.getpid()} {appcursor}"
                mode = r"printf '\033[?1h'; " if appcursor else ""
                command = f"stty raw -echo; {mode}printf R; exec cat"
                terminal = subprocess.Popen(
                    [str(ROOT / "worminal"), "-T", title, "-o", "-",
                     "-e", "/bin/sh", "-c", command],
                    env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                )
                try:
                    deadline = time.monotonic() + 5
                    window = None
                    while time.monotonic() < deadline:
                        result = subprocess.run(
                            ["xdotool", "search", "--onlyvisible", "--name", title],
                            env=env, capture_output=True, text=True,
                        )
                        if result.returncode == 0:
                            window = result.stdout.splitlines()[0]
                            break
                        time.sleep(0.05)
                    self.assertIsNotNone(window, "Worminal did not open an X11 window")
                    self.assertEqual(read_bytes(terminal.stdout, 6 if appcursor else 1),
                                     b"\033[?1hR" if appcursor else b"R")
                    subprocess.run(["xdotool", "windowfocus", "--sync", window],
                                   env=env, check=True)
                    for keys, normal, application in (
                        ("Home", b"\033[H", b"\033OH"),
                        ("End", b"\033[F", b"\033OF"),
                    ):
                        with self.subTest(appcursor=appcursor, keys=keys):
                            subprocess.run(["xdotool", "key", "--clearmodifiers", keys],
                                           env=env, check=True)
                            expected = application if appcursor else normal
                            self.assertEqual(read_bytes(terminal.stdout, len(expected)), expected)
                    for keys, expected in cases:
                        with self.subTest(appcursor=appcursor, keys=keys):
                            subprocess.run(["xdotool", "key", "--clearmodifiers", keys],
                                           env=env, check=True)
                            self.assertEqual(read_bytes(terminal.stdout, len(expected)), expected)
                finally:
                    if terminal.poll() is None:
                        terminal.terminate()
                    terminal.communicate(timeout=2)
