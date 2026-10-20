"""Preview protocol records, version coexistence and PTY backpressure limits."""

from functools import partial
from itertools import product
import os
import shlex
from pathlib import Path
import socket
import struct
import subprocess
import time
import unittest
import uuid

from workspace_protocol import number, string, send as wire_send, wait_kind as wire_wait, packet as wire_packet

ROOT = Path(__file__).resolve().parent.parent
DAEMON = ROOT / os.environ.get("WORMINAL_TEST_PREVIEW_DAEMON", "worminald-preview")
send = partial(wire_send, version=2)
wait_kind = partial(wire_wait, version=2)


def records(data):
    def integer():
        nonlocal data
        value = struct.unpack("!I", data[:4])[0]
        data = data[4:]
        return value

    def text():
        nonlocal data
        length = integer()
        value, data = data[:length].decode(), data[length:]
        return value

    count = integer()
    integer()
    integer()
    result = []
    for _ in range(count):
        tab, kind, url = integer(), integer(), text()
        mode, cursor, title, directory = integer(), integer(), text(), text()
        for _ in range(integer()):
            text()
        result.append((tab, kind, url, mode, cursor, title, directory))
    if data:
        raise AssertionError("extra catalog data")
    return result


@unittest.skipUnless(DAEMON.exists(), "run manage.py build-preview first")
class PreviewWorkspaceTest(unittest.TestCase):
    def setUp(self):
        self.scope = f"preview-{os.getpid()}-{uuid.uuid4().hex}"
        self.env = {**os.environ, "WORMINAL_SHARED_SOCKET_SCOPE": self.scope, "WORMINAL_INHERIT_TEST": "inherited"}
        self.legacy = socket.socket(socket.AF_UNIX)
        self.legacy.bind(f"\0worminal-workspace-{os.getuid()}-{self.scope}")
        self.legacy.listen(1)
        self.legacy.settimeout(.05)
        self.addCleanup(self.legacy.close)
        self.daemon = subprocess.Popen([str(DAEMON), "--serve"], env=self.env,
                                       stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        self.addCleanup(self.stop)
        self.peer = self.connect()
        self.addCleanup(self.peer.close)

    def connect(self):
        deadline = time.monotonic() + 3
        while True:
            peer = socket.socket(socket.AF_UNIX)
            peer.settimeout(2)
            try:
                peer.connect(f"\0worminal-workspace-v2-{os.getuid()}-{self.scope}")
                return peer
            except OSError:
                peer.close()
                if time.monotonic() > deadline:
                    self.fail("preview service did not start")
                time.sleep(.01)

    def stop(self):
        self.daemon.terminate()
        self.daemon.communicate(timeout=3)

    def launch(self, command):
        args = ["/bin/sh", "-c", command]
        launch = b"".join((number(40), number(4), number(1), number(len(args)), number(0),
                           string("test"), string(str(ROOT)), string(""), string("-"), string(""),
                           *(string(arg) for arg in args)))
        send(self.peer, 1, payload=launch)
        tab, _ = wait_kind(self.peer, 20)
        wait_kind(self.peer, 7)
        output = b""
        marker = b"READY:inherited" if "WORMINAL_INHERIT_TEST" in command else b"READY"
        while marker not in output:
            _, chunk = wait_kind(self.peer, 14, tab)
            output += chunk
        if "WORMINAL_INHERIT_TEST" in command:
            self.assertIn(b"READY:inherited", output)
        return tab

    def test_browser_records_have_no_pty_and_saved_links_survive_attach(self):
        send(self.peer, 17, payload=string("https://example.invalid/first"))
        tab, _ = wait_kind(self.peer, 20)
        _, catalog = wait_kind(self.peer, 7)
        self.assertEqual(records(catalog)[0][:3], (tab, 1, "https://example.invalid/first"))
        children = Path(f"/proc/{self.daemon.pid}/task/{self.daemon.pid}/children").read_text().strip()
        self.assertEqual(children, "")
        # Terminal-only operations cannot manufacture a PTY for a browser record.
        send(self.peer, 4, tab, number(100) + number(20))
        send(self.peer, 3, tab, b"ignored")
        send(self.peer, 6, tab, number(1) + number(3))
        with self.connect() as peer:
            send(peer, 19)
            _, catalog = wait_kind(peer, 7)
            self.assertEqual(len(records(catalog)), 1)
            send(peer, 18, tab, string("https://example.invalid/second"))
            _, catalog = wait_kind(self.peer, 7)
            self.assertEqual(records(catalog)[0][2], "https://example.invalid/second")
        terminal = self.launch("stty raw -echo; printf READY; exec cat")
        send(self.peer, 5, tab)
        _, catalog = wait_kind(self.peer, 7)
        self.assertEqual([(row[0], row[1]) for row in records(catalog)], [(terminal, 0)])
        with self.assertRaises(socket.timeout):
            self.legacy.accept()

    def test_remote_environment_and_deferred_input_order(self):
        command = ('stty raw -echo; printf "READY:$WORMINAL_INHERIT_TEST"; sleep .1; '
                   'dd bs=1024 count=64 iflag=fullblock 2>/dev/null | sha256sum; sleep 1')
        tab = self.launch(command)
        import hashlib
        payload = bytes(range(256)) * 256
        send(self.peer, 3, tab, payload)
        output = b""
        digest = hashlib.sha256(payload).hexdigest().encode()
        while digest not in output:
            _, chunk = wait_kind(self.peer, 14, tab)
            output += chunk
        self.assertIn(digest, output)

    def test_command_converts_calling_terminal_and_preserves_order(self):
        for executable, url in product((DAEMON, ROOT / "worminal"), ("https://example.invalid/converted", None)):
            with self.subTest(executable=executable.name, url=url):
                command = shlex.quote(str(executable)) + " --new-browser-tab"
                if url: command += " " + shlex.quote(url)
                tab = self.launch("stty raw -echo; printf READY; dd bs=1 count=1 >/dev/null 2>&1; "
                                  + command + "; sleep 30")
                other = self.launch("stty raw -echo; printf READY; exec cat")
                send(self.peer, 3, tab, b"x")
                for _ in range(512):
                    _, payload = wait_kind(self.peer, 7)
                    rows = records(payload)
                    converted = next((row for row in rows if row[0] == tab and row[1] == 1), None)
                    if converted:
                        self.assertEqual(converted[2], url or "about:blank")
                        identities = [row[0] for row in rows]
                        self.assertLess(identities.index(tab), identities.index(other))
                        self.assertEqual(next(row for row in rows if row[0] == other)[1], 0)
                        break
                else:
                    self.fail("calling terminal was not converted")
                send(self.peer, 3, other, b"still-live")
                output = b""
                while b"still-live" not in output:
                    _, chunk = wait_kind(self.peer, 14, other)
                    output += chunk

    def test_browser_command_outside_a_terminal_is_rejected(self):
        for executable in (DAEMON, ROOT / "worminal"):
            with self.subTest(executable=executable.name):
                env = {key: value for key, value in self.env.items()
                       if key not in ("DISPLAY", "WAYLAND_DISPLAY")}
                result = subprocess.run([str(executable), "--new-browser-tab"], env=env,
                                        capture_output=True, timeout=5)
                self.assertEqual(result.returncode, 1)
                self.assertIn(b"Open worminal", result.stderr)
                self.assertIn(b"keep WORMINAL_SHARED_SOCKET_SCOPE unchanged", result.stderr)
                self.assertNotIn(b"Invalid browser URL", result.stderr)

    def test_empty_browser_url_keeps_the_calling_terminal(self):
        command = shlex.quote(str(DAEMON)) + " --new-browser-tab ''"
        tab = self.launch("stty raw -echo; printf READY; dd bs=1 count=1 >/dev/null 2>&1; "
                          + command + "; exec cat")
        send(self.peer, 3, tab, b"x")
        output = b""
        while b"Invalid browser URL" not in output:
            _, chunk = wait_kind(self.peer, 14, tab)
            output += chunk
        send(self.peer, 19)
        _, payload = wait_kind(self.peer, 7)
        self.assertEqual([(row[0], row[1]) for row in records(payload)], [(tab, 0)])
        self.assertNotIn(b"Open worminal", output)

    def test_input_reply_frame_precedes_its_mode_catalog(self):
        # Catalog titles are also frontend readiness barriers. A client must not
        # see a new input mode while its imported alternate screen is stale.
        code = ("import os\nos.write(1,b'READY')\n"
                "for i in range(12):\n"
                " os.read(0,1)\n"
                " mode = 'h' if i%2==0 else 'l'\n"
                " os.write(1,f'\\x1b[?1049{mode}\\x1b]2;mode-{i}\\a'.encode())\n"
                "os.read(0,1)\n")
        tab = self.launch("stty raw -echo; exec python3 -u -c " + shlex.quote(code))
        alternate = 0
        for index in range(12):
            send(self.peer, 3, tab, b"x")
            for _ in range(512):
                kind, identity, payload = wire_packet(self.peer, version=2)
                if identity == tab and kind == 8:
                    alternate = struct.unpack("!6I", payload)[5]
                if kind == 7 and records(payload)[0][5] == f"mode-{index}":
                    self.assertEqual(alternate, int(index % 2 == 0),
                                     "mode catalog preceded its corresponding frame")
                    break
            else:
                self.fail("missing mode catalog")

    def test_stale_resize_cannot_reclaim_another_controller(self):
        tab = self.launch("stty raw -echo; printf READY; exec cat")
        with self.connect() as peer:
            send(peer, 19)
            wait_kind(peer, 7)
            send(peer, 4, tab, number(40) + number(4))
            wait_kind(peer, 10, tab)
            wait_kind(self.peer, 15, tab)
            send(self.peer, 23, tab, number(39) + number(4))
            send(self.peer, 19)
            wait_kind(self.peer, 7)  # Same-stream barrier after the stale allocation.
            send(peer, 19)
            kind, _, _ = wire_packet(peer, version=2)
            self.assertEqual(kind, 7, "stale resize released the new controller")

    def test_queue_saturation_stops_only_affected_tab(self):
        blocked = self.launch("stty raw -echo; printf READY; exec sleep 30")
        for _ in range(18):
            send(self.peer, 3, blocked, b"x" * 65536)
        wait_kind(self.peer, 16, blocked)
        _, catalog = wait_kind(self.peer, 7)
        self.assertEqual(records(catalog), [])
        tab = self.launch("stty raw -echo; printf READY; exec cat")
        send(self.peer, 3, tab, b"responsive")
        output = b""
        while b"responsive" not in output:
            _, chunk = wait_kind(self.peer, 14, tab)
            output += chunk
        self.assertIsNone(self.daemon.poll())
