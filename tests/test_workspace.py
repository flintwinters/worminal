"""Exercise the headless workspace through its framed local socket."""

import os
from pathlib import Path
import select
import socket
import struct
import subprocess
import time
import unittest
import uuid


from workspace_protocol import number, string, send, wait_kind, read_exact


ROOT = Path(__file__).resolve().parent.parent
DAEMON = ROOT / os.environ.get("WORMINAL_TEST_WORKSPACE_DAEMON", "worminald-x11")
HELLO, INPUT, FOCUS, CATALOG, FRAME, ROW, FINISH, CLIPBOARD, STOP, PRINT = 1, 3, 4, 7, 8, 9, 10, 11, 13, 14


class WorkspaceServiceTest(unittest.TestCase):
    def test_bridge_congestion_does_not_block_opposite_direction(self):
        for congested_output in (True, False):
            with self.subTest(congested_output=congested_output):
                scope = f"bridge-congestion-{os.getpid()}-{uuid.uuid4().hex}"
                env = {**os.environ, "WORMINAL_SHARED_SOCKET_SCOPE": scope}
                with socket.socket(socket.AF_UNIX) as listener:
                    endpoint = ("worminal-workspace-v2" if os.environ.get("WORMINAL_TEST_BRIDGE_VERSION") == "2"
                                else "worminal-workspace")
                    listener.bind(f"\0{endpoint}-{os.getuid()}-{scope}")
                    listener.listen(1)
                    listener.settimeout(3)
                    local, remote = socket.socketpair()
                    remote.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, 4096)
                    process = subprocess.Popen([str(DAEMON), "--bridge"], env=env,
                                               stdin=remote, stdout=remote, stderr=subprocess.PIPE)
                    remote.close()
                    try:
                        with local, listener.accept()[0] as service:
                            sender, receiver = (service, local) if congested_output else (local, service)
                            sender.setblocking(False)
                            payload = bytes(range(256)) * 16384
                            sent = 0
                            deadline = time.monotonic() + 3
                            # Fill the transport and bridge buffers without consuming
                            # the destination. Readiness settling proves backpressure.
                            while sent < len(payload) and time.monotonic() < deadline:
                                if not select.select([], [sender], [], .2)[1]:
                                    break
                                try:
                                    sent += sender.send(payload[sent:])
                                except BlockingIOError:
                                    continue
                            self.assertGreater(sent, 0)
                            self.assertLess(sent, len(payload), "fixture did not saturate bridge")
                            sender.settimeout(2)
                            receiver.settimeout(2)
                            marker = b"opposite-direction-still-responsive"
                            receiver.sendall(marker)
                            self.assertEqual(read_exact(sender, len(marker)), marker)
                            # Draining resumes exact, ordered delivery after short writes.
                            self.assertEqual(read_exact(receiver, sent), payload[:sent])
                    finally:
                        local.close()
                        process.terminate()
                        process.communicate(timeout=3)

    def test_blocked_pty_does_not_pause_new_shells_or_parser_replies(self):
        scope = f"blocked-{os.getpid()}-{uuid.uuid4().hex}"
        env = {**os.environ, "WORMINAL_SHARED_SOCKET_SCOPE": scope}
        service = subprocess.Popen([str(DAEMON), "--serve"], env=env,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        peers = []
        try:
            for index, command in enumerate(("stty raw -echo; printf R; exec sleep 30",
                                             "stty raw -echo; printf R; exec cat")):
                deadline = time.monotonic() + 3
                while True:
                    peer = socket.socket(socket.AF_UNIX)
                    peer.settimeout(2)
                    try:
                        peer.connect(f"\0worminal-workspace-{os.getuid()}-{scope}")
                        break
                    except OSError:
                        peer.close()
                        if time.monotonic() > deadline: self.fail("workspace did not start")
                        time.sleep(.01)
                peers.append(peer)
                payload = b"".join((number(20), number(4), number(1), number(3), number(0),
                                    string("backpressure"), string(str(ROOT)), string(""), string("-"),
                                    string(""), string("/bin/sh"), string("-c"), string(command)))
                send(peer, HELLO, payload=payload)
                _, catalog = wait_kind(peer, CATALOG)
                printed = b""
                while b"R" not in printed:
                    tab, chunk = wait_kind(peer, PRINT)
                    printed += chunk
                if index == 0:
                    # Saturate a PTY whose application never reads. ttywriteraw used
                    # to block inside pselect here, preventing every subsequent launch.
                    send(peer, INPUT, tab, b"x" * 65536)
                else:
                    send(peer, INPUT, tab, b"responsive")
                    printed = b""
                    while b"responsive" not in printed:
                        _, chunk = wait_kind(peer, PRINT, tab)
                        printed += chunk
                    self.assertIn(b"responsive", printed)
                    send(peer, INPUT, tab, b"\x1b[6n")
                    printed = b""
                    while b"R" not in printed:
                        _, chunk = wait_kind(peer, PRINT, tab)
                        printed += chunk
                    self.assertRegex(printed, br"\x1b\[[0-9]+;[0-9]+R")
            self.assertIsNone(service.poll())
        finally:
            for peer in peers: peer.close()
            service.terminate()
            service.communicate(timeout=3)

    def test_partial_client_packet_does_not_pause_other_tab(self):
        scope = f"partial-{os.getpid()}-{uuid.uuid4().hex}"
        env = {**os.environ, "WORMINAL_SHARED_SOCKET_SCOPE": scope}
        address = f"\0worminal-workspace-{os.getuid()}-{scope}"
        service = subprocess.Popen([str(DAEMON), "--serve"], env=env,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        connections = []
        try:
            for index in range(2):
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
                launch = b"".join((number(40), number(10), number(1), number(1), number(0),
                                   string(f"partial-{index}"), string(str(ROOT)), string(""),
                                   string("-"), string(""), string("/bin/cat")))
                send(connection, HELLO, payload=launch)
                _, catalog = wait_kind(connection, CATALOG)
                if index == 0:
                    first_tab = struct.unpack("!I", catalog[12:16])[0]

            # A readable socket with an incomplete header used to hold the service
            # in wire_read until its receive timeout, delaying every other PTY.
            partial = struct.pack("!4I", 1, INPUT, first_tab, 3)
            connections[1].sendall(partial[:8])
            connections[0].settimeout(1.25)
            send(connections[0], INPUT, first_tab, b"responsive\n")
            printed = b""
            while b"responsive" not in printed:
                _, chunk = wait_kind(connections[0], PRINT)
                printed += chunk
            self.assertIn(b"responsive", printed)
            connections[1].sendall(partial[8:] + b"y")
            send(connections[0], INPUT, first_tab, b"second\n")
            printed = b""
            while b"second" not in printed:
                _, chunk = wait_kind(connections[0], PRINT)
                printed += chunk
            self.assertIn(b"second", printed)
            connections[1].sendall(b"z\n")
            printed = b""
            while b"yz" not in printed:
                _, chunk = wait_kind(connections[0], PRINT)
                printed += chunk
            self.assertIn(b"yz", printed)
        finally:
            for connection in reversed(connections):
                connection.close()
            subprocess.run([str(DAEMON), "--stop"], env=env,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=3)
            try:
                service.communicate(timeout=3)
            except subprocess.TimeoutExpired:
                service.kill()
                service.communicate()

    def test_bridge_starts_service_and_preserves_tabs_between_connections(self):
        scope = f"bridge-{os.getpid()}-{uuid.uuid4().hex}"
        env = {**os.environ, "WORMINAL_SHARED_SOCKET_SCOPE": scope}
        bridges = []
        try:
            for index in range(2):
                local, remote = socket.socketpair()
                local.settimeout(3)
                process = subprocess.Popen([str(DAEMON), "--bridge"],
                                           env=env, stdin=remote, stdout=remote,
                                           stderr=subprocess.PIPE)
                remote.close()
                bridges.append((process, local))
                command = (["/bin/sh", "-c", "printf '\\033]52;c;Q09ERVg=\\a'; exec cat"]
                           if index == 0 else ["/bin/cat"])
                launch = b"".join((number(40), number(10), number(1), number(len(command)), number(0),
                                   string(f"bridge-{index}"), string(str(ROOT)), string(""),
                                   string("-"), string(""), *(string(arg) for arg in command)))
                send(local, HELLO, payload=launch)
                _, catalog = wait_kind(local, CATALOG)
                self.assertEqual(struct.unpack("!I", catalog[:4])[0], index + 1)
                if index == 0:
                    _, clipboard = wait_kind(local, CLIPBOARD)
                    self.assertEqual(clipboard, string("CODEX"))
                    local.shutdown(socket.SHUT_WR)
                    process.wait(timeout=3)
                    self.assertEqual(process.returncode, 0)
        finally:
            subprocess.run([str(DAEMON), "--stop"], env=env,
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
        service = subprocess.Popen([str(DAEMON), "--serve"], env=env,
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
            # The next frame must be for the resize. A redundant focus used to
            # send a full 40-column screen before the requested 41-column one.
            send(second, FOCUS, first_tab, number(40) + number(10))
            send(second, FOCUS, first_tab, number(41) + number(10))
            _, frame = wait_kind(second, FRAME, first_tab)
            self.assertEqual(struct.unpack("!I", frame[:4])[0], 41)
            wait_kind(second, FINISH, first_tab)
            send(second, INPUT, first_tab, b"shared\n")
            printed = b""
            while b"shared" not in printed and len(printed) < 128:
                _, chunk = wait_kind(connections[0], PRINT)
                printed += chunk
            self.assertIn(b"shared", printed)
            connections[0].close()
            connections.pop(0)
            send(second, FOCUS, first_tab, number(41) + number(10))
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
