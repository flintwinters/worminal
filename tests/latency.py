"""Measure process launch to the first injected key echoed through st's PTY.

Xvfb starts before timing. Each trial launches Worminal with /bin/cat, polls for
its window, injects one key, and observes that byte through st's -o - output.
This includes launch, mapping, keyboard dispatch, and PTY output processing;
it excludes Xvfb startup and the user's shell startup. No host display is used.
"""

import os
from pathlib import Path
import select
import statistics
import subprocess
import time

from x11 import isolated_display


ROOT = Path(__file__).resolve().parent.parent
TRIALS = 20
TIMEOUT = 5


def measure_one(env, index):
    title = f"Worminal latency {os.getpid()} {index}"
    terminal = None
    try:
        started = time.monotonic_ns()
        terminal = subprocess.Popen(
            [str(ROOT / "worminal"), "-T", title, "-o", "-", "-e", "/bin/cat"],
            cwd=ROOT,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        deadline = time.monotonic() + TIMEOUT
        injected = False
        while time.monotonic() < deadline:
            if terminal.poll() is not None:
                break
            if not injected:
                # A sync search dominated the measured latency in trial runs.
                # Short explicit polls keep that wait out of the measurement.
                result = subprocess.run(
                    ["xdotool", "search", "--onlyvisible", "--name", title,
                     "windowfocus", "key", "--clearmodifiers", "z"],
                    env=env,
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.PIPE,
                )
                injected = result.returncode == 0
                if not injected:
                    time.sleep(0.001)
                    continue
            ready, _, _ = select.select([terminal.stdout], [], [], max(0, min(0.1, deadline - time.monotonic())))
            if ready:
                data = os.read(terminal.stdout.fileno(), 4096)
                if b"z" in data:
                    return (time.monotonic_ns() - started) / 1_000_000
                if not data:
                    break
            if terminal.poll() is not None:
                break
        error = terminal.stderr.read().decode() if terminal.poll() is not None else ""
        raise RuntimeError(f"Trial {index}: first key was not echoed within {TIMEOUT}s: {error}".strip())
    finally:
        if terminal is not None:
            if terminal.poll() is None:
                terminal.terminate()
            try:
                terminal.communicate(timeout=2)
            except subprocess.TimeoutExpired:
                terminal.kill()
                terminal.communicate()


def main():
    with isolated_display() as env:
        samples = [measure_one(env, index) for index in range(TRIALS)]
    ordered = sorted(samples)
    p95 = ordered[(95 * TRIALS + 99) // 100 - 1]
    print(
        f"Launch to first key echo: median {statistics.median(samples):.1f} ms, "
        f"p95 {p95:.1f} ms (n={TRIALS}, private Xvfb, /bin/cat)"
    )


if __name__ == "__main__":
    main()
