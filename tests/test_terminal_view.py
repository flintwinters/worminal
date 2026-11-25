"""Drive the terminal-only frontend and a real daemon with a private PTY."""
from runtime import CHECKS, runtime_binary

import fcntl
import os
from pathlib import Path
import select
import shlex
import socket
import struct
import subprocess
import tempfile
import termios
import time
import unittest
import uuid


DAEMON = runtime_binary(os.environ.get("WORMINAL_TEST_WORKSPACE_DAEMON", "worminald"))


def controlling_tty():
    os.setsid()
    fcntl.ioctl(0, termios.TIOCSCTTY, 0)


def stop(process):
    if process.poll() is None:
        process.terminate()
    process.communicate(timeout=5)


class TerminalViewTest(unittest.TestCase):
    def setUp(self):
        CHECKS.mkdir(parents=True, exist_ok=True)
        work = tempfile.TemporaryDirectory(prefix="terminal-view-", dir=CHECKS)
        self.addCleanup(work.cleanup)
        root = self.root = Path(work.name)
        for name in ("alpha", "beta"):
            (root / name).mkdir()
        self.env = {
            **os.environ, "HOME": str(root), "SHELL": "/bin/sh", "TERM": "xterm-256color",
            "XDG_STATE_HOME": str(root / "state"), "XDG_CONFIG_HOME": str(root / "config"),
            "WORMINAL_SHARED_SOCKET_SCOPE": f"terminal-view-{uuid.uuid4().hex}",
        }
        service = subprocess.Popen([DAEMON, "--serve"], cwd=root, env=self.env,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        self.addCleanup(stop, service)
        deadline = time.monotonic() + 5
        with socket.socket(socket.AF_UNIX) as peer:
            address = f"\0worminal-workspace-v2-{os.getuid()}-{self.env['WORMINAL_SHARED_SOCKET_SCOPE']}"
            while True:
                try:
                    peer.connect(address)
                    break
                except ConnectionRefusedError:
                    if service.poll() is not None or time.monotonic() >= deadline:
                        raise
                    time.sleep(.01)
        self.master, self.slave = os.openpty()
        self.addCleanup(os.close, self.master)
        self.addCleanup(os.close, self.slave)
        self.resize(80, 24)
        self.original = termios.tcgetattr(self.slave)

    def resize(self, columns, rows):
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, columns, 0, 0))

    def open_view(self):
        process = subprocess.Popen([DAEMON, "terminal"], env=self.env, stdin=self.slave,
                                   stdout=self.slave, stderr=self.slave, preexec_fn=controlling_tty)
        self.addCleanup(stop, process)
        self.wait_for(b"\x1b[?25h")
        return process

    def wait_for(self, expected):
        output = bytearray()
        deadline = time.monotonic() + 6
        while expected not in output:
            if not select.select([self.master], [], [], max(0, deadline - time.monotonic()))[0]:
                self.fail(f"Missing terminal output {expected!r}, received {bytes(output[-1200:])!r}")
            output.extend(os.read(self.master, 65536))
        return output

    def send(self, data):
        os.write(self.master, data)

    def detach(self, process):
        self.send(b"\x1d" + b"q")
        self.wait_for(b"\x1b[?1049l")
        self.assertEqual(process.wait(timeout=5), 0)
        self.assertEqual(termios.tcgetattr(self.slave), self.original)

    def test_shell_resize_switch_and_reattach(self):
        view = self.open_view()
        self.send(b"retained=first; printf 'ready-%s\\n' \"$retained\"\r")
        self.wait_for(b"ready-first")
        self.resize(67, 19)
        # The bar occupies row 1 and the 18-row PTY ends at row 19.
        self.wait_for(b"\x1b[19;1H")
        self.send(b"stty size\r")
        self.wait_for(b"18 67")

        self.send(b"\x1dt")
        self.wait_for(b"\x1b[19;1H")
        self.send(b"retained=second; printf 'ready-%s\\n' \"$retained\"\r")
        self.wait_for(b"ready-second")
        self.send(b"\x1dp")
        self.wait_for(b"ready-first")
        self.send(b"\x1dn")
        self.wait_for(b"ready-second")
        self.detach(view)

        view = self.open_view()
        self.send(b"printf 'reattach-%s\\n' \"$retained\"\r")
        self.wait_for(b"reattach-first")
        self.detach(view)

    def test_tab_bar_order_selection_and_narrow_view(self):
        view = self.open_view()
        self.send(f"cd {shlex.quote(str(self.root / 'alpha'))}; printf '\\n'\r".encode())
        self.wait_for(b"\x1b[7m1 alpha \x1b[0m")
        self.send(b"\x1dt")
        self.wait_for(b"\x1b[7m2 ")
        self.send(f"cd {shlex.quote(str(self.root / 'beta'))}; printf '\\n'\r".encode())
        bar = self.wait_for(b"\x1b[7m2 beta \x1b[0m")
        self.assertIn(b"1 alpha \x1b[0m \x1b[7m2 beta ", bar)
        self.send(b"\x1dp")
        bar = self.wait_for(b"\x1b[7m1 alpha \x1b[0m")
        self.assertIn(b"\x1b[7m1 alpha \x1b[0m 2 beta ", bar)
        self.send(b"\x1dn")
        self.wait_for(b"\x1b[7m2 beta \x1b[0m")
        self.resize(8, 1)
        narrow_bar = b"\x1b[1;1H\x1b[0m\x1b[2K\x1b[7m2 beta "
        bar = self.wait_for(narrow_bar)
        self.assertNotIn(b"\x1b[2;1H", bar.split(narrow_bar, 1)[1])
        self.resize(8, 2)
        self.wait_for(b"\x1b[2;1H")
        self.detach(view)

    def test_bracketed_paste_keeps_command_prefix_literal(self):
        view = self.open_view()
        self.send(
            b"stty raw -echo; printf '\\033[?2004h'; "
            b"dd bs=1 count=14 2>/dev/null | od -An -tx1; stty sane\r"
        )
        self.wait_for(b"\x1b[?2004h")
        self.send(b"\x1b[200")
        self.send(b"~\x1dq\x1b[201~")
        self.wait_for(b"1d 71")
        self.detach(view)
