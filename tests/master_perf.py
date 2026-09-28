"""Measure tab-switch frame traffic through an isolated SSH workspace service."""

import os
import select
import shlex
import struct
import subprocess
import sys
import time
import uuid


HELLO, NEW, FOCUS, FRAME, FINISH = 1, 2, 4, 8, 10
SSH = ["ssh", "-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=8"]


def number(value):
    return struct.pack("!I", value)


def string(value):
    encoded = value.encode()
    return number(len(encoded)) + encoded


def send(process, kind, tab=0, payload=b""):
    process.stdin.write(struct.pack("!4I", 1, kind, tab, len(payload)) + payload)
    process.stdin.flush()


def read_exact(process, count, deadline):
    data = bytearray()
    while len(data) < count:
        remaining = deadline - time.monotonic()
        if remaining <= 0 or not select.select([process.stdout], [], [], remaining)[0]:
            raise TimeoutError("workspace frame did not arrive within 20 seconds")
        part = os.read(process.stdout.fileno(), count - len(data))
        if not part:
            raise RuntimeError(f"SSH bridge closed (status {process.poll()})")
        data.extend(part)
    return bytes(data)


def receive(process, deadline):
    version, kind, tab, length = struct.unpack("!4I", read_exact(process, 16, deadline))
    if version != 1 or length > 8 * 1024 * 1024:
        raise RuntimeError("invalid workspace packet")
    return kind, tab, read_exact(process, length, deadline), 16 + length


def launch(process, kind):
    payload = b"".join((number(120), number(40), number(1), number(1), number(0),
                        string("master-perf"), string(""), string(""), string(""),
                        string(""), string("/bin/cat")))
    started = time.monotonic()
    send(process, kind, payload=payload)
    deadline = time.monotonic() + 20
    tab = None
    total = 0
    while True:
        packet_kind, packet_tab, _, size = receive(process, deadline)
        if packet_kind == FRAME:
            tab = packet_tab
        if tab == packet_tab and tab is not None:
            total += size
        if packet_kind == FINISH and packet_tab == tab:
            return tab, total, (time.monotonic() - started) * 1000


def measure(host, daemon, compress):
    scope = f"master-perf-{os.getpid()}-{uuid.uuid4().hex}"
    prefix = f"env WORMINAL_SHARED_SOCKET_SCOPE={shlex.quote(scope)} {shlex.quote(daemon)}"
    ssh = [*SSH, "-C"] if compress else SSH
    process = subprocess.Popen([*ssh, host, f"{prefix} --bridge"], stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        first, initial_bytes, initial_ms = launch(process, HELLO)
        second, new_bytes, new_ms = launch(process, NEW)
        started = time.monotonic()
        for tab in (first, second, first):
            send(process, FOCUS, tab, number(120) + number(40))
        send(process, FOCUS, first, number(121) + number(40))
        deadline = time.monotonic() + 20
        redundant_bytes = 0
        redundant_frames = 0
        while True:
            kind, tab, payload, size = receive(process, deadline)
            if kind == FRAME and tab == first and struct.unpack("!I", payload[:4])[0] == 121:
                break
            redundant_bytes += size
            redundant_frames += kind == FRAME
        delay_ms = (time.monotonic() - started) * 1000
        while True:
            kind, tab, _, _ = receive(process, deadline)
            if kind == FINISH and tab == first:
                break
        print(f"{daemon} ({'SSH -C' if compress else 'SSH'}): "
              f"initial 120x40 frame {initial_bytes:,} bytes / {initial_ms:.1f} ms; "
              f"new tab {new_bytes:,} bytes / {new_ms:.1f} ms; "
              f"three cached switches {redundant_frames} frames / {redundant_bytes:,} bytes; "
              f"resize frame began after {delay_ms:.1f} ms")
    finally:
        subprocess.run([*ssh, host, f"{prefix} --stop"], stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL, timeout=12)
        process.terminate()
        try:
            process.communicate(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.communicate()


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("Usage: python3 manage.py master-perf HOST")
    daemon = os.environ.get("WORMINAL_MASTER_DAEMON", "worminald")
    for compress in (False, True):
        measure(sys.argv[1], daemon, compress)
