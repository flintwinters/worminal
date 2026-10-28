"""Measure tab-switch frame traffic through an isolated SSH workspace service."""

import os
import select
import shlex
import statistics
import struct
import subprocess
import sys
import time
import uuid


HELLO, NEW, FOCUS, FRAME, FINISH = 1, 2, 4, 8, 10
VERSION = int(os.environ.get("WORMINAL_MASTER_PROTOCOL", "2"))
SSH = ["ssh", "-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=8"]


def number(value):
    return struct.pack("!I", value)


def string(value):
    encoded = value.encode()
    return number(len(encoded)) + encoded


def send(process, kind, tab=0, payload=b""):
    process.stdin.write(struct.pack("!4I", VERSION, kind, tab, len(payload)) + payload)
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
    if version != VERSION or length > 8 * 1024 * 1024:
        raise RuntimeError("invalid workspace packet")
    return kind, tab, read_exact(process, length, deadline), 16 + length


def launch(process, kind, command=None):
    command = command or ["/bin/cat"]
    payload = b"".join((number(120), number(40), number(1), number(len(command)), number(0),
                        string("master-perf"), string(""), string(""), string(""),
                        string(""), *(string(arg) for arg in command)))
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


def echo_frame(process, tab, expected, row=0):
    matched = False
    total = 0
    deadline = time.monotonic() + 20
    while True:
        kind, identity, payload, size = receive(process, deadline)
        if identity != tab:
            continue
        total += size
        if kind == 9 and struct.unpack_from("!I", payload)[0] == row:
            text = ''.join(chr(struct.unpack_from("!I", payload, offset)[0])
                           for offset in range(4, len(payload), 16))
            matched = text.startswith(expected)
        if kind == FINISH and matched:
            return total


def summary(label, samples):
    ordered = sorted(samples)
    print(f"{label}: median {statistics.median(samples):.2f} ms; "
          f"p95 {ordered[(95 * len(samples) + 99) // 100 - 1]:.2f} ms", flush=True)


def scroll_command():
    code = ("import os,random\nr=random.Random(42)\n"
            "def line(): return bytes(r.randrange(33,127) for _ in range(100))\n"
            "os.write(1,b'\\r\\n'.join([line() for _ in range(39)]+[b'READY']))\n"
            "for i in range(40):\n"
            " os.read(0,1)\n"
            " os.write(1,b'\\r\\n'+f'ACK{i:06d}'.encode()+line()+b'~END~')\n"
            "os.read(0,1)\n")
    return "stty raw -echo rows 40 cols 120; exec python3 -u -c " + shlex.quote(code)


def ordinary_echo(host, scroll=False):
    command = scroll_command() if scroll else "stty raw -echo; printf READY; exec cat"
    process = subprocess.Popen([*SSH, "-tt", host, command],
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        ready = b""
        deadline = time.monotonic() + 20
        while not ready.endswith(b"READY"):
            ready += read_exact(process, 1, deadline)
        samples = []
        for _ in range(40):
            started = time.monotonic_ns()
            process.stdin.write(b"x")
            process.stdin.flush()
            deadline = time.monotonic() + 20
            if scroll:
                response = b""
                while not response.endswith(b"~END~"):
                    response += read_exact(process, 1, deadline)
            elif read_exact(process, 1, deadline) != b"x":
                raise RuntimeError("ordinary SSH did not echo the expected byte")
            samples.append((time.monotonic_ns() - started) / 1e6)
        summary("Ordinary SSH " + ("scrolling echo" if scroll else "PTY echo"), samples)
    finally:
        process.terminate()
        process.communicate(timeout=3)


def measure(host, daemon, compress):
    scope = f"master-perf-{os.getpid()}-{uuid.uuid4().hex}"
    prefix = f"env WORMINAL_SHARED_SOCKET_SCOPE={shlex.quote(scope)} {shlex.quote(daemon)}"
    ssh = [*SSH, "-C"] if compress else SSH
    process = subprocess.Popen([*ssh, host, f"{prefix} --bridge"], stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        if VERSION == 2:
            send(process, 28, payload=number(1))
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
              f"resize frame began after {delay_ms:.1f} ms", flush=True)
        tab, _, _ = launch(process, NEW, ["/bin/sh", "-c", "stty raw -echo; printf READY; exec cat"])
        expected = "READY"
        echo_frame(process, tab, expected)
        samples = []
        for _ in range(40):
            expected += "x"
            started = time.monotonic_ns()
            send(process, 3, tab, b"x")
            echo_frame(process, tab, expected)
            samples.append((time.monotonic_ns() - started) / 1e6)
        summary(f"Worminal input to complete frame ({'SSH -C' if compress else 'SSH'})", samples)
        if compress:
            tab, _, _ = launch(process, NEW, ["/bin/sh", "-c", scroll_command()])
            echo_frame(process, tab, "READY", row=39)
            samples, traffic = [], []
            for index in range(40):
                started = time.monotonic_ns()
                send(process, 3, tab, b"x")
                traffic.append(echo_frame(process, tab, f"ACK{index:06d}", row=39))
                samples.append((time.monotonic_ns() - started) / 1e6)
            summary("Worminal scrolling input to complete frame (SSH -C)", samples)
            print(f"Protocol bytes per scrolling response: median {statistics.median(traffic):.0f}; "
                  "application PTY output: 116 bytes", flush=True)
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
    ordinary_echo(sys.argv[1])
    ordinary_echo(sys.argv[1], scroll=True)
    for compress in (False, True):
        measure(sys.argv[1], daemon, compress)
