"""Check navigation keys through X11, the PTY, and a raw-mode child."""

import os
from pathlib import Path
import select
import shutil
import subprocess
import time
import unittest

from x11 import isolated_display


ROOT = Path(__file__).resolve().parent.parent
FRONTEND = os.environ.get("WORMINAL_FRONTEND", "worminal")
PREVIEW = FRONTEND == "worminal-preview"
DAEMON = "worminald-preview" if PREVIEW else "worminald"


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


def read_through(stream, final):
    data = b""
    while len(data) < 40 and not data.endswith(final):
        chunk = read_bytes(stream, 1)
        if not chunk:
            break
        data += chunk
    return data


def wait_title(env, window, title):
    # PTY print output precedes parser/catalog updates. The title is a frontend
    # acknowledgment that the same catalog carrying the input modes was applied.
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        actual = subprocess.check_output(["xdotool", "getwindowname", window], env=env, text=True).strip()
        if actual == title:
            return
        time.sleep(.01)
    raise AssertionError("frontend did not apply the ready input modes")


class NavigationKeysTest(unittest.TestCase):
    def test_alternate_scroll_sends_cursor_keys(self):
        self.check_alternate_scroll([])

    def test_master_alternate_scroll_sends_cursor_keys(self):
        self.check_alternate_scroll(["--master", "wheel-test"])

    def check_alternate_scroll(self, connection):
        with isolated_display() as env:
            if connection:
                fixtures = ROOT / "tests/fixtures" / ("preview-master" if PREVIEW else "master")
                env["PATH"] = str(fixtures) + os.pathsep + env["PATH"]
                env["WORMINAL_TEST_DAEMON"] = str(ROOT / DAEMON)
            for appcursor, mouse_report in ((False, False), (True, False), (False, True)):
                title = f"Worminal alternate scroll {os.getpid()} {appcursor} {mouse_report}"
                mode = b"\033[?1049h\033[?1007h"
                if appcursor:
                    mode += b"\033[?1h"
                if mouse_report:
                    mode += b"\033[?1000h\033[?1006h"
                mode += f"\033]2;{title} ready\a".encode()
                escapes = "".join(f"\\{byte:03o}" for byte in mode)
                command = f"stty raw -echo; printf '{escapes}R'; exec cat"
                terminal = subprocess.Popen(
                    [str(ROOT / FRONTEND), *connection, "-T", title, "-o", "-",
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
                    if window is None:
                        status = terminal.poll()
                        ready, _, _ = select.select([terminal.stderr], [], [], 0)
                        detail = os.read(terminal.stderr.fileno(), 4096).decode() if ready else ""
                        display = subprocess.run(["xdotool", "getdisplaygeometry"], env=env,
                                                 capture_output=True, text=True)
                        self.fail(f"Worminal did not open an X11 window: exit={status}, "
                                  f"display={env['DISPLAY']} server={display.returncode} "
                                  f"stderr={detail!r}")
                    wait_title(env, window, title + " ready")
                    self.assertEqual(read_bytes(terminal.stdout, len(mode) + 1), mode + b"R")
                    subprocess.run(["xdotool", "windowfocus", "--sync", window],
                                   env=env, check=True)
                    subprocess.run(["xdotool", "mousemove", "--window", window, "20", "80"],
                                   env=env, check=True)
                    for button, normal, application in (
                        ("4", b"\033[A", b"\033OA"),
                        ("5", b"\033[B", b"\033OB"),
                    ):
                        with self.subTest(appcursor=appcursor, mouse_report=mouse_report,
                                          button=button):
                            subprocess.run(["xdotool", "click", button], env=env, check=True)
                            if mouse_report:
                                code = 64 if button == "4" else 65
                                self.assertRegex(read_through(terminal.stdout, b"M").decode(),
                                                 rf"^\x1b\[<{code};[0-9]+;[0-9]+M$")
                            else:
                                expected = application if appcursor else normal
                                self.assertEqual(read_bytes(terminal.stdout, len(expected)), expected)
                finally:
                    if terminal.poll() is None:
                        terminal.terminate()
                    terminal.communicate(timeout=2)

    @unittest.skipUnless(shutil.which("micro"), "micro is not installed")
    def test_ctrl_end_in_micro(self):
        path = ROOT / "build" / f"micro-key-test-{os.getpid()}.txt"
        log = ROOT / "build" / f"micro-output-{os.getpid()}"
        config = ROOT / "build" / f"micro-config-{os.getpid()}"
        config.mkdir()
        path.write_text("first\nlast")
        title = f"Worminal micro keys {os.getpid()}"
        with isolated_display() as env:
            terminal = subprocess.Popen(
                [str(ROOT / FRONTEND), "-T", title, "-o", str(log), "-e", "micro",
                 "-config-dir", str(config), str(path)],
                env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
            )
            try:
                deadline = time.monotonic() + 5
                window = None
                while time.monotonic() < deadline:
                    result = subprocess.run(
                        ["xdotool", "search", "--onlyvisible", "--class",
                         "worminal-preview" if PREVIEW else "Worminal"],
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
                    raise AssertionError(
                        f"micro did not exit; file contains {path.read_text()!r}; "
                        f"output {log.read_bytes()[:300]!r}"
                    ) from error
                self.assertEqual(path.read_text(), "QYfirstX\nlastZ\n")
            finally:
                if terminal.poll() is None:
                    terminal.terminate()
                terminal.communicate(timeout=2)
                path.unlink(missing_ok=True)
                log.unlink(missing_ok=True)
                shutil.rmtree(config)

    def test_navigation_keys(self):
        self.check_navigation_keys([])

    def test_master_navigation_keys(self):
        self.check_navigation_keys(["--master", "key-test"])

    def check_navigation_keys(self, connection):
        cases = (
            ("Delete", b"\033[3~"),
            ("ctrl+Delete", b"\033[3;5~"),
            ("shift+Delete", b"\033[3;2~"),
            ("KP_Delete", b"\033[3~"),
            ("ctrl+KP_Delete", b"\033[3;5~"),
            ("shift+KP_Delete", b"\033[3;2~"),
            ("ctrl+End", b"\033[1;5F"),
            ("ctrl+Home", b"\033[1;5H"),
            ("shift+End", b"\033[1;2F"),
            ("shift+Home", b"\033[1;2H"),
            ("ctrl+shift+End", b"\033[1;6F"),
        )
        with isolated_display() as env:
            if connection:
                fixtures = ROOT / "tests/fixtures" / ("preview-master" if PREVIEW else "master")
                env["PATH"] = str(fixtures) + os.pathsep + env["PATH"]
                env["WORMINAL_TEST_DAEMON"] = str(ROOT / DAEMON)
            for appcursor, appkeypad in ((False, False), (False, True), (True, False), (True, True)):
                title = f"Worminal keys {os.getpid()} {appcursor} {appkeypad}"
                mode = (b"\033[?1h" if appcursor else b"") + (b"\033=" if appkeypad else b"")
                mode += f"\033]2;{title} ready\a".encode()
                escapes = "".join(f"\\{byte:03o}" for byte in mode)
                command = f"stty raw -echo; printf '{escapes}R'; exec cat"
                terminal = subprocess.Popen(
                    [str(ROOT / FRONTEND), *connection, "-T", title, "-o", "-",
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
                    if window is None:
                        detail = terminal.stderr.read(4096) if terminal.poll() is not None else b""
                        self.fail(f"Worminal did not open: exit={terminal.poll()}, stderr={detail!r}")
                    wait_title(env, window, title + " ready")
                    self.assertEqual(read_bytes(terminal.stdout, len(mode) + 1), mode + b"R")
                    focused = subprocess.run(["xdotool", "windowfocus", "--sync", window],
                                             env=env, capture_output=True)
                    if focused.returncode:
                        detail = terminal.stderr.read(4096) if terminal.poll() is not None else b""
                        self.fail(f"Worminal exited before focus: {terminal.poll()}, {detail!r}, "
                                  f"{focused.stderr!r}")
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
