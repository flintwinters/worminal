"""Measure real SSH -C wire bytes of hidden busy tabs through an isolated remote workspace."""

import os
import select
import shlex
import subprocess
import sys
import time
import uuid

from master_perf import FOCUS, SSH, launch, number, receive, send

COLUMNS, ROWS, BUSY, SECONDS = 187, 82, 5, 20
# COPY_ROWS | ROW_SPAN, optionally FOCUSED_FRAMES (bit 4). Older frontends advertise only the first two.
FEATURES = {"current frontend": 1 | 8 | 16, "pre-FOCUSED_FRAMES frontend": 1 | 8}
# A status line redrawn about 20 times per second, like an agent TUI's spinner and timer.
SPINNER = ["/bin/sh", "-c", 'while :; do printf "\\r working %s" "$(date +%s%N)"; sleep 0.05; done']


def wire_bytes(pid):
    """Bytes the local SSH client received on its TCP connection (compressed, encrypted, no IP/TCP headers)."""
    out = subprocess.run(["ss", "-tinpH"], capture_output=True, text=True).stdout.split("\n")
    for index, line in enumerate(out):
        if f"pid={pid}," in line:
            for field in out[index + 1].split():
                if field.startswith("bytes_received:"):
                    return int(field.split(":")[1])
    raise RuntimeError("SSH connection not found")


def drain(process, seconds):
    protocol, deadline = 0, time.monotonic() + seconds
    while (remaining := deadline - time.monotonic()) > 0:
        if select.select([process.stdout], [], [], remaining)[0]:
            protocol += receive(process, time.monotonic() + 20)[3]
    return protocol


def measure(host, daemon, label, features):
    scope = f"master-traffic-{os.getpid()}-{uuid.uuid4().hex}"
    prefix = f"env WORMINAL_SHARED_SOCKET_SCOPE={shlex.quote(scope)} {shlex.quote(daemon)}"
    ssh = [*SSH, "-C"]
    process = subprocess.Popen([*ssh, host, f"{prefix} --bridge"], stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        send(process, 28, payload=number(features))
        visible, _, _ = launch(process, 1, columns=COLUMNS, rows=ROWS)
        busy = [launch(process, 2, SPINNER, COLUMNS, ROWS)[0] for _ in range(BUSY)]
        for tab in (*busy, visible):  # Every tab activated once stays controlled, as in a real window.
            send(process, FOCUS, tab, number(COLUMNS) + number(ROWS))
        drain(process, 3)
        before = wire_bytes(process.pid)
        protocol = drain(process, SECONDS)
        wire = wire_bytes(process.pid) - before
        print(f"{label}: {BUSY} hidden busy {COLUMNS}x{ROWS} tabs over {SECONDS} s: "
              f"wire {wire / SECONDS / 1024:.1f} KiB/s ({wire / SECONDS * 3600 / 2**20:.1f} MiB/h), "
              f"protocol {protocol / SECONDS / 1024:.1f} KiB/s", flush=True)
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
        raise SystemExit("Usage: python3 manage.py master-traffic HOST")
    daemon = os.environ.get("WORMINAL_MASTER_DAEMON", "worminald")
    for label, features in FEATURES.items():
        measure(sys.argv[1], daemon, label, features)
