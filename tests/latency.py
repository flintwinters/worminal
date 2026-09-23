"""Measure process launch to the first injected key echoed through st's PTY.

Xvfb starts before timing. A persistent X11 client waits for each new window,
injects one key, and the probe observes it through st's -o - output.
This includes launch, mapping, keyboard dispatch, and PTY output processing.
The optional shell mode also measures interactive shell startup. Neither mode
includes Xvfb startup or uses the host display.
"""

import os
from pathlib import Path
import select
import statistics
import subprocess
import sys
import time

from x11 import isolated_display


ROOT = Path(__file__).resolve().parent.parent
TRIALS = 20
TIMEOUT = 5
PHASES = ("main", "display", "pty", "fontconfig", "font", "map-request", "mapped", "resize", "key", "echo")
SHELL_MARKER = b"__WORMINAL_SHELL_READY__"


def measure_one(env, index, shell=False):
    title = f"Worminal latency {os.getpid()} {index}"
    terminal = None
    injector = None
    trace = b""
    injector_error = b""
    injector_status = None
    finished = None
    started = None
    try:
        env = dict(env, WORMINAL_TRACE_TIMING="1")
        # Keep a one-shot shell alive briefly so st can drain the marker from
        # its PTY before SIGCHLD ends the terminal process.
        shell_command = (
            'IFS= read -r value; [ "$value" = z ] && '
            f"printf '{SHELL_MARKER.decode()}\\n'; sleep 0.2"
        )
        command = (
            [env.get("SHELL", "/bin/sh"), "-i", "-c",
             shell_command]
            if shell else ["/bin/cat"]
        )
        expected = SHELL_MARKER if shell else b"z"
        injector = subprocess.Popen(
            [str(ROOT / ".checks/key_injector"), title] + (["Return"] if shell else []), env=env,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        ready, _, _ = select.select([injector.stdout], [], [], TIMEOUT)
        if not ready or injector.stdout.readline() != b"ready\n":
            raise RuntimeError(f"Trial {index}: key injector did not become ready")
        started = time.monotonic_ns()
        terminal = subprocess.Popen(
            [str(ROOT / "worminal"), "-T", title, "-o", "-", "-e", *command],
            cwd=ROOT,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        deadline = time.monotonic() + TIMEOUT
        output = b""
        while time.monotonic() < deadline:
            ready, _, _ = select.select([terminal.stdout], [], [], max(0, min(0.1, deadline - time.monotonic())))
            if ready:
                data = os.read(terminal.stdout.fileno(), 4096)
                output += data
                if expected in output:
                    finished = time.monotonic_ns()
                    break
                if not data:
                    break
    finally:
        if terminal is not None:
            if terminal.poll() is None:
                terminal.terminate()
            try:
                _, trace = terminal.communicate(timeout=2)
            except subprocess.TimeoutExpired:
                terminal.kill()
                _, trace = terminal.communicate()
        if injector is not None:
            if injector.poll() is None:
                injector.terminate()
            try:
                _, injector_error = injector.communicate(timeout=2)
            except subprocess.TimeoutExpired:
                injector.kill()
                _, injector_error = injector.communicate()
            injector_status = injector.returncode

    if finished is None:
        raise RuntimeError(
            f"Trial {index}: input response was not observed within {TIMEOUT}s "
            f"(injector status {injector_status}): output={output[-512:]!r} "
            f"injector={injector_error.decode()} trace={trace.decode()}"
        )
    points = {"start": started, "echo": finished}
    for line in trace.splitlines():
        parts = line.split()
        if len(parts) == 3 and parts[0] == b"worminal-timing":
            points[parts[1].decode()] = int(parts[2])
    missing = set(PHASES) - points.keys()
    if missing:
        raise RuntimeError(f"Trial {index}: missing timing phases {sorted(missing)}: {trace.decode()}")
    return {phase: (points[phase] - started) / 1_000_000 for phase in PHASES}


def main():
    shell = sys.argv[1:] == ["--shell"]
    if sys.argv[1:] and not shell:
        raise SystemExit("Usage: latency.py [--shell]")
    with isolated_display() as env:
        samples = [measure_one(env, index, shell=shell) for index in range(TRIALS)]
    ordered = sorted(sample["echo"] for sample in samples)
    p95 = ordered[(95 * TRIALS + 99) // 100 - 1]
    print(
        f"Launch to {'shell input read' if shell else 'first key echo'}: "
        f"median {statistics.median(ordered):.1f} ms, "
        f"p95 {p95:.1f} ms (n={TRIALS}, private Xvfb, "
        f"{os.environ.get('SHELL', '/bin/sh') if shell else '/bin/cat'})"
    )
    previous = "start"
    for phase in PHASES:
        duration = statistics.median(sample[phase] - sample.get(previous, 0) for sample in samples)
        print(f"  {previous} -> {phase}: {duration:.1f} ms")
        previous = phase


if __name__ == "__main__":
    main()
