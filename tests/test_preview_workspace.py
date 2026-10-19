"""Preview protocol records, version coexistence and PTY backpressure limits."""

from functools import partial
import os
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
