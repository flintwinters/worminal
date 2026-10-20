"""Small framed workspace test client; versions stay explicit at call sites."""

import struct


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


def packet(connection, version=1):
    actual_version, kind, tab, length = struct.unpack("!4I", read_exact(connection, 16))
    if actual_version != version or length > 8 * 1024 * 1024:
        raise AssertionError("invalid workspace packet")
    return kind, tab, read_exact(connection, length)


def send(connection, kind, tab=0, payload=b"", version=1):
    connection.sendall(struct.pack("!4I", version, kind, tab, len(payload)) + payload)


def wait_kind(connection, wanted, wanted_tab=None, version=1):
    for _ in range(512):
        kind, tab, payload = packet(connection, version)
        if kind == wanted and (wanted_tab is None or tab == wanted_tab):
            return tab, payload
    raise AssertionError(f"workspace did not send packet {wanted}")
