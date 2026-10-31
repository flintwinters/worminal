"""Small framed workspace test client; versions stay explicit at call sites."""

import struct
import os
from pathlib import Path


def rust_daemon(executable):
    """Select implementation-specific fault injection; custom C proof binaries remain C by default."""
    explicit = os.environ.get("WORMINAL_TEST_RUST_DAEMON")
    if explicit is not None:
        return explicit == "1"
    root = Path(__file__).resolve().parent.parent
    return Path(executable).resolve() in {root / "worminald", root / "worminald-preview",
                                         root / "build/cargo/release/worminal-daemon"}


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


def catalog_records(data, version=2):
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
        tab = integer()
        kind, url = (integer(), text()) if version == 2 else (0, "")
        mode, cursor, title, directory = integer(), integer(), text(), text()
        for _ in range(integer()):
            text()
        result.append((tab, kind, url, mode, cursor, title, directory))
    if data:
        raise AssertionError("extra catalog data")
    return result


def service_pid(connection, version=2):
    # An inherited listening socket retains its creator's SO_PEERCRED PID.
    # Ask the accepting daemon instead of mistaking that stale PID for its owner.
    send(connection, 30, version=version)
    return struct.unpack("!I", wait_kind(connection, 30, version=version)[1])[0]
