"""Probe the single-owner socket and last-view lifecycle proposed for shared tabs."""

import multiprocessing
import os
from pathlib import Path
import socket
import unittest


ROOT = Path(__file__).resolve().parent.parent


def owner(path, ready):
    count = 0
    with socket.socket(socket.AF_UNIX) as server:
        server.bind(str(path))
        server.listen()
        server.settimeout(5)
        ready.send(True)
        while True:
            with server.accept()[0] as client:
                command = client.recv(1)
                if command == b"A":
                    count += 1
                elif command == b"D" and count:
                    count -= 1
                else:
                    raise AssertionError("invalid view lifecycle command")
                client.sendall(bytes([count]))
                if command == b"D" and count == 0:
                    break
    path.unlink()


def send(path, command):
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(2)
        client.connect(str(path))
        client.sendall(command)
        return client.recv(1)


class OwnerLifecycleTest(unittest.TestCase):
    def test_two_launchers_one_owner_until_last_view_closes(self):
        path = ROOT / ".checks" / f"owner-proof-{os.getpid()}.sock"
        path.unlink(missing_ok=True)
        receive, transmit = multiprocessing.Pipe(duplex=False)
        process = multiprocessing.Process(target=owner, args=(path, transmit))
        process.start()
        transmit.close()
        try:
            self.assertTrue(receive.poll(5), "owner did not start")
            self.assertTrue(receive.recv())
            self.assertEqual(send(path, b"A"), b"\x01")
            with socket.socket(socket.AF_UNIX) as competitor:
                with self.assertRaises(OSError):
                    competitor.bind(str(path))
            self.assertEqual(send(path, b"A"), b"\x02")
            self.assertEqual(send(path, b"D"), b"\x01")
            self.assertTrue(process.is_alive(), "first close ended the owner")
            self.assertEqual(send(path, b"D"), b"\x00")
            process.join(5)
            self.assertEqual(process.exitcode, 0)
            self.assertFalse(path.exists(), "owner left a stale socket")
        finally:
            if process.is_alive():
                process.terminate()
                process.join(2)
            receive.close()
            path.unlink(missing_ok=True)
