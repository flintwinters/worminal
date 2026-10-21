"""Local ownership transfers use real PTYs, isolated scopes, and both daemon ABIs."""
import array
import hashlib
import os
from pathlib import Path
import socket
import shlex
import signal
import struct
import subprocess
import time
import unittest
import uuid

from workspace_protocol import number, string, send, wait_kind

ROOT = Path(__file__).resolve().parent.parent
BINARIES = {1: ROOT / "worminald-x11", 2: ROOT / "worminald-preview"}


def catalog_rows(payload, version):
    offset = 12
    rows = []

    def integer():
        nonlocal offset
        result, = struct.unpack_from("!I", payload, offset)
        offset += 4
        return result

    def text():
        nonlocal offset
        size = integer()
        result = payload[offset:offset + size].decode()
        offset += size
        return result

    for _ in range(struct.unpack_from("!I", payload)[0]):
        tab = integer()
        if version == 2:
            integer()
            text()
        integer()
        integer()
        title, cwd = text(), text()
        for _ in range(integer()):
            text()
        rows.append((tab, title, cwd))
    return rows


class HandoffTest(unittest.TestCase):
    def setUp(self):
        self.services = []
        self.peers = []
        self.token = f"test-{os.getpid()}-{uuid.uuid4().hex}"

    def tearDown(self):
        for peer in self.peers:
            peer.close()
        # Stop through the protocol so the service hangs up owned shells and closes PTYs.
        for service, version, env in reversed(self.services):
            if service.poll() is None:
                subprocess.run([str(BINARIES[version]), "--stop"], env=env, timeout=3,
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            try:
                service.communicate(timeout=3)
            except subprocess.TimeoutExpired:
                service.kill()
                service.communicate(timeout=3)

    def start(self, version, binary=None):
        scope = f"handoff-{uuid.uuid4().hex}"
        env = {**os.environ, "WORMINAL_SHARED_SOCKET_SCOPE": scope}
        service = subprocess.Popen([str(binary or BINARIES[version]), "--serve"], env=env,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        self.services.append((service, version, env))
        prefix = "worminal-workspace-v2" if version == 2 else "worminal-workspace"
        deadline = time.monotonic() + 3
        while True:
            peer = socket.socket(socket.AF_UNIX)
            peer.settimeout(3)
            try:
                peer.connect(f"\0{prefix}-{os.getuid()}-{scope}")
                self.peers.append(peer)
                return peer, env, service
            except OSError:
                peer.close()
                if time.monotonic() >= deadline:
                    self.fail(f"service failed to start: {service.poll()}")
                time.sleep(.01)

    def command(self, version, env, flag, okay=True):
        result = subprocess.run([str(BINARIES[version]), flag, self.token], env=env,
                                capture_output=True, timeout=12)
        if okay:
            self.assertEqual(result.returncode, 0, result.stderr.decode())
        else:
            self.assertNotEqual(result.returncode, 0)
        return result

    def launch(self, peer, version, title, command=None):
        command = command or 'stty raw -echo; printf "READY:%s\\r\\n" "$$"; exec cat'
        launch = b"".join((number(40), number(8), number(1), number(3), number(0),
                           string(title), string(str(ROOT)), string(""), string("-"),
                           string(""), string("/bin/sh"), string("-c"), string(command)))
        send(peer, 1, payload=launch, version=version)
        _, catalog = wait_kind(peer, 7, version=version)
        tab = catalog_rows(catalog, version)[-1][0]
        printed = b""
        while b"READY:" not in printed or b"\n" not in printed:
            _, chunk = wait_kind(peer, 14, tab, version=version)
            printed += chunk
        pid = int(printed.split(b"READY:", 1)[1].split()[0])
        return tab, pid

    def assert_screen(self, peer, version, tab, expected):
        for _ in range(128):
            _, row = wait_kind(peer, 9, tab, version=version)
            cells = [struct.unpack_from("!I", row, offset)[0] for offset in range(4, len(row), 16)]
            text = "".join(chr(c) for c in cells)
            if expected in text:
                return
        self.fail(f"screen did not contain {expected!r}")

    def test_live_round_trips_merge_without_replacing_shells(self):
        pairs = [(source, destination, None, None) for source, destination in ((1, 2), (2, 1), (1, 1), (2, 2))]
        baseline = os.environ.get("WORMINAL_TEST_HANDOFF_BASELINE")
        if baseline:
            self.assertTrue(Path(baseline).is_file())
            pairs.extend(((2, 2, baseline, None), (2, 2, None, baseline)))
        for source, destination, source_binary, target_binary in pairs:
            with self.subTest(source=source, destination=destination, old_donor=bool(source_binary),
                              old_receiver=bool(target_binary)):
                donor, source_env, donor_process = self.start(source, source_binary)
                receiver, target_env, _ = self.start(destination, target_binary)
                _, existing_pid = self.launch(receiver, destination, "existing")
                tab, pid = self.launch(donor, source, "incoming")
                send(donor, 3, tab, b"before-transfer", version=source)
                self.assert_screen(donor, source, tab, "before-transfer")
                self.command(destination, target_env, "--handin")
                self.command(source, source_env, "--handoff")
                _, catalog = wait_kind(receiver, 7, version=destination)
                records = catalog_rows(catalog, destination)
                self.assertEqual([r[1] for r in records], ["existing", "incoming"])
                self.assertEqual(records[1][2], str(ROOT))
                imported = records[1][0]
                self.assertNotEqual(records[0][0], imported)
                # Ownership has moved: the donor retains no PTY master descriptors.
                donor_fds = Path(f"/proc/{donor_process.pid}/fdinfo")
                self.assertFalse(any("tty-index:" in p.read_text() for p in donor_fds.iterdir()))
                # The same child and PTY survive, including when the original parent exits.
                self.assertTrue(Path(f"/proc/{pid}").exists())
                self.assertTrue(Path(f"/proc/{existing_pid}").exists())
                subprocess.run([str(BINARIES[source]), "--stop"], env=source_env, check=True, timeout=3)
                donor_process.wait(timeout=3)
                send(receiver, 4, imported, number(40) + number(8), version=destination)
                self.assert_screen(receiver, destination, imported, "before-transfer")
                send(receiver, 3, imported, b" after-transfer", version=destination)
                self.assert_screen(receiver, destination, imported, "after-transfer")

    def test_receiver_owns_shutdown_of_imported_shell(self):
        for source, destination in ((1, 2), (2, 1)):
            with self.subTest(source=source, destination=destination):
                donor, env, parent = self.start(source)
                receiver, target_env, _ = self.start(destination)
                self.launch(receiver, destination, "existing")
                _, pid = self.launch(donor, source, "owned")
                self.command(destination, target_env, "--handin")
                self.command(source, env, "--handoff")
                _, payload = wait_kind(receiver, 7, version=destination)
                imported = catalog_rows(payload, destination)[-1][0]
                send(receiver, 5, imported, version=destination)
                deadline = time.monotonic() + 3
                while Path(f"/proc/{pid}").exists():
                    if time.monotonic() >= deadline:
                        self.fail("receiver did not terminate its imported shell")
                    time.sleep(.01)
                self.assertIsNone(parent.poll())

    def test_pending_input_survives_handoff_without_loss_or_duplication(self):
        donor, env, _ = self.start(1)
        receiver, target_env, _ = self.start(2)
        self.launch(receiver, 2, "existing")
        payload = b"queued-input-" * 16000
        code = ("import os, hashlib, time\n"
                "print('READY:%s' % os.getpid(), flush=True)\n"
                "data = b''\n"
                f"while len(data) < {len(payload)}:\n"
                f" data += os.read(0, {len(payload)} - len(data))\n"
                "print('HASH:' + hashlib.sha256(data).hexdigest()[:16], flush=True)\n"
                "time.sleep(30)\n")
        command = "stty raw -echo; exec python3 -u -c " + shlex.quote(code)
        tab, pid = self.launch(donor, 1, "queued", command)
        os.kill(pid, signal.SIGSTOP)
        try:
            deadline = time.monotonic() + 3
            while "State:\tT" not in Path(f"/proc/{pid}/status").read_text():
                if time.monotonic() >= deadline:
                    self.fail("test reader did not stop")
                time.sleep(.01)
            for offset in range(0, len(payload), 65536):
                send(donor, 3, tab, payload[offset:offset + 65536])
            send(donor, 4, tab, number(41) + number(8))
            for _ in range(32):
                _, frame = wait_kind(donor, 8, tab)
                if struct.unpack_from("!I", frame)[0] == 41:
                    break
            else:
                self.fail("input queue barrier was not processed")
            self.command(2, target_env, "--handin")
            self.command(1, env, "--handoff")
            _, catalog = wait_kind(receiver, 7, version=2)
            imported = catalog_rows(catalog, 2)[-1][0]
            send(receiver, 4, imported, number(40) + number(8), version=2)
        finally:
            os.kill(pid, signal.SIGCONT)
        expected = "HASH:" + hashlib.sha256(payload).hexdigest()[:16]
        self.assert_screen(receiver, 2, imported, expected)

    def test_unavailable_receiver_retains_live_shell(self):
        for version in (1, 2):
            with self.subTest(version=version):
                peer, env, _ = self.start(version)
                tab, pid = self.launch(peer, version, "retained")
                self.command(version, env, "--handoff", okay=False)
                self.assertTrue(Path(f"/proc/{pid}").exists())
                send(peer, 3, tab, b"still-working", version=version)
                self.assert_screen(peer, version, tab, "still-working")

    def test_receiver_rejects_truncated_snapshot_and_can_then_receive(self):
        receiver, target_env, _ = self.start(2)
        self.launch(receiver, 2, "existing")
        self.command(2, target_env, "--handin")
        # A same-user malformed sender must not publish partial tabs or consume the listener.
        with socket.socket(socket.AF_UNIX, socket.SOCK_SEQPACKET) as peer:
            peer.settimeout(3)
            peer.connect(f"\0worminal-handoff-{os.getuid()}-{self.token}")
            fd = os.memfd_create("invalid-handoff", os.MFD_ALLOW_SEALING)
            try:
                peer.sendmsg([b"H"], [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array("i", [fd]))])
                self.assertEqual(peer.recv(1), b"")
            finally:
                os.close(fd)
        donor, source_env, _ = self.start(1)
        self.launch(donor, 1, "incoming")
        self.command(1, source_env, "--handoff")
        _, payload = wait_kind(receiver, 7, version=2)
        self.assertEqual([r[1] for r in catalog_rows(payload, 2)], ["existing", "incoming"])

    def test_v1_rejects_browser_records_without_removing_them(self):
        donor, env, _ = self.start(2)
        self.launch(donor, 2, "terminal")
        send(donor, 17, payload=string("https://example.invalid/"), version=2)
        wait_kind(donor, 7, version=2)
        _, target_env, _ = self.start(1)
        self.command(1, target_env, "--handin")
        self.command(2, env, "--handoff", okay=False)
        # Query the donor's catalog through a fresh v2 attach.
        send(donor, 19, version=2)
        _, payload = wait_kind(donor, 7, version=2)
        self.assertEqual(len(catalog_rows(payload, 2)), 2)

    def test_receiver_disconnect_before_ready_rolls_back_donor(self):
        donor, env, _ = self.start(1)
        tab, pid = self.launch(donor, 1, "retained")
        address = f"\0worminal-handoff-{os.getuid()}-{self.token}"
        with socket.socket(socket.AF_UNIX, socket.SOCK_SEQPACKET) as listener:
            listener.bind(address)
            listener.listen(1)
            process = subprocess.Popen([str(BINARIES[1]), "--handoff", self.token], env=env,
                                       stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            listener.settimeout(3)
            peer, _ = listener.accept()
            with peer:
                _, control, _, _ = peer.recvmsg(1, socket.CMSG_SPACE(201 * 4))
                for level, kind, data in control:
                    if level == socket.SOL_SOCKET and kind == socket.SCM_RIGHTS:
                        values = array.array("i")
                        values.frombytes(data)
                        for fd in values:
                            os.close(fd)
            process.communicate(timeout=3)
            self.assertNotEqual(process.returncode, 0)
        self.assertTrue(Path(f"/proc/{pid}").exists())
        send(donor, 3, tab, b"rollback-working", version=1)
        self.assert_screen(donor, 1, tab, "rollback-working")
