"""Exercise the headless workspace through its framed local socket."""

import os
from pathlib import Path
import socket
import struct
import subprocess
import time
import unittest
import uuid


ROOT = Path(__file__).resolve().parent.parent
HELLO, INPUT, FOCUS, CATALOG, FRAME, ROW, FINISH, STOP, PRINT = 1, 3, 4, 7, 8, 9, 10, 13, 14


def number(value):
    return struct.pack("!I", value)


def string(value):
    encoded = value.encode()
    return number(len(encoded)) + encoded


def read_exact(connection, count):
    data = bytearray()
    while len(data) < count:
        part = connection.recv(count - len(data))
        if not part:
            raise AssertionError("workspace closed its connection")
        data.extend(part)
    return bytes(data)


def packet(connection):
    version, kind, tab, length = struct.unpack("!4I", read_exact(connection, 16))
    if version != 1 or length > 8 * 1024 * 1024:
        raise AssertionError("invalid workspace packet")
    return kind, tab, read_exact(connection, length)


def send(connection, kind, tab=0, payload=b""):
    connection.sendall(struct.pack("!4I", 1, kind, tab, len(payload)) + payload)


def wait_kind(connection, wanted, wanted_tab=None):
    for _ in range(512):
        kind, tab, payload = packet(connection)
        if kind == wanted and (wanted_tab is None or tab == wanted_tab):
            return tab, payload
    raise AssertionError(f"workspace did not send packet {wanted}")


class WorkspaceServiceTest(unittest.TestCase):
    def test_bridge_starts_service_and_preserves_tabs_between_connections(self):
        scope = f"bridge-{os.getpid()}-{uuid.uuid4().hex}"
        env = {**os.environ, "WORMINAL_SHARED_SOCKET_SCOPE": scope}
        bridges = []
        try:
            for index in range(2):
                local, remote = socket.socketpair()
                local.settimeout(3)
                process = subprocess.Popen([str(ROOT / "worminald"), "--bridge"],
                                           env=env, stdin=remote, stdout=remote,
                                           stderr=subprocess.PIPE)
                remote.close()
                bridges.append((process, local))
                launch = b"".join((number(40), number(10), number(1), number(1), number(0),
                                   string(f"bridge-{index}"), string(str(ROOT)), string(""),
                                   string("-"), string(""), string("/bin/cat")))
                send(local, HELLO, payload=launch)
                _, catalog = wait_kind(local, CATALOG)
                self.assertEqual(struct.unpack("!I", catalog[:4])[0], index + 1)
                if index == 0:
                    local.shutdown(socket.SHUT_WR)
                    process.wait(timeout=3)
                    self.assertEqual(process.returncode, 0)
        finally:
            subprocess.run([str(ROOT / "worminald"), "--stop"], env=env,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            for process, connection in bridges:
                connection.close()
                if process.poll() is None:
                    process.terminate()
                process.communicate(timeout=3)

    def test_two_clients_share_a_tab_and_service_survives_disconnect(self):
        scope = f"wire-{os.getpid()}-{uuid.uuid4().hex}"
        env = {**os.environ, "WORMINAL_SHARED_SOCKET_SCOPE": scope}
        address = f"\0worminal-workspace-{os.getuid()}-{scope}"
        service = subprocess.Popen([str(ROOT / "worminald"), "--serve"], env=env,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        connections = []
        try:
            for _ in range(2):
                deadline = time.monotonic() + 3
                while True:
                    connection = socket.socket(socket.AF_UNIX)
                    connection.settimeout(3)
                    try:
                        connection.connect(address)
                        break
                    except OSError:
                        connection.close()
                        if time.monotonic() >= deadline:
                            self.fail(f"workspace did not start: {service.poll()}")
                        time.sleep(.01)
                connections.append(connection)
                title = f"tab-{len(connections)}"
                launch = b"".join((number(40), number(10), number(1), number(1), number(0),
                                   string(title), string(str(ROOT)), string(""), string("-"),
                                   string(""), string("/bin/cat")))
                send(connection, HELLO, payload=launch)
                _, catalog = wait_kind(connection, CATALOG)
                self.assertEqual(struct.unpack("!I", catalog[:4])[0], len(connections))
            first_tab = struct.unpack("!I", catalog[12:16])[0]
            second = connections[1]
            send(second, FOCUS, first_tab, number(40) + number(10))
            wait_kind(second, FINISH, first_tab)
            send(second, INPUT, first_tab, b"shared\n")
            printed = b""
            while b"shared" not in printed and len(printed) < 128:
                _, chunk = wait_kind(connections[0], PRINT)
                printed += chunk
            self.assertIn(b"shared", printed)
            connections[0].close()
            connections.pop(0)
            send(second, FOCUS, first_tab, number(40) + number(10))
            wait_kind(second, FINISH, first_tab)
            self.assertIsNone(service.poll())
        finally:
            if connections:
                send(connections[0], STOP)
            for connection in connections:
                connection.close()
            try:
                service.communicate(timeout=3)
            except subprocess.TimeoutExpired:
                service.kill()
                service.communicate()


if __name__ == "__main__":
    unittest.main()
