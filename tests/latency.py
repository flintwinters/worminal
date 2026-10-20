"""Measure process launch to the first injected key echoed through st's PTY.

Xvfb starts before timing. A persistent X11 client waits for each new window,
injects one key, and the probe observes it through st's -o - output.
This includes launch, mapping, keyboard dispatch, and PTY output processing.
The optional shell mode also measures interactive shell startup. Neither mode
includes Xvfb startup or uses the host display.
"""

import argparse
import json
import os
from pathlib import Path
import select
import statistics
import subprocess
import time

from x11 import close_window, isolated_display


ROOT = Path(__file__).resolve().parent.parent
TRIALS = 20
TIMEOUT = 5
PHASES = ("main", "workspace", "display", "map-request", "fontconfig", "font",
          "map-observed", "resize", "key", "echo")
GTK_PHASES = ("main", "broker", "gtk", "map-request", "workspace", "font", "echo")
SHELL_MARKER = b"__WORMINAL_SHELL_READY__"


def measure_one(env, index, shell=False):
    title = f"Worminal latency {os.getpid()} {index}"
    terminal = None
    injector = None
    trace = b""
    injector_error = b""
    injector_output = b""
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
            [str(ROOT / "build/key_injector"), title] + (["Return"] if shell else []), env=env,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        ready, _, _ = select.select([injector.stdout], [], [], TIMEOUT)
        if not ready or injector.stdout.readline() != b"ready\n":
            raise RuntimeError(f"Trial {index}: key injector did not become ready")
        started = time.monotonic_ns()
        terminal = subprocess.Popen(
            [str(ROOT / env.get("WORMINAL_FRONTEND", "worminal")), "-T", title, "-o", "-", "-e", *command],
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
        if terminal is not None and env.get("WORMINAL_LATENCY_FORWARDED"):
            try:
                close_window(env, title, TIMEOUT)
            except (subprocess.CalledProcessError, subprocess.TimeoutExpired, IndexError):
                pass  # A failed launch may never have mapped its window.
        if terminal is not None:
            if terminal.poll() is None:
                terminal.terminate()
            try:
                _, trace = terminal.communicate(timeout=2)
            except subprocess.TimeoutExpired:
                terminal.kill()
                if env.get("WORMINAL_LATENCY_FORWARDED"):
                    # The owner may still hold the launcher's stdout descriptor.
                    terminal.stdout.close()
                    terminal.stderr.close()
                    terminal.wait(timeout=2)
                else:
                    _, trace = terminal.communicate(timeout=2)
        if injector is not None:
            if injector.poll() is None:
                injector.terminate()
            try:
                injector_output, injector_error = injector.communicate(timeout=2)
            except subprocess.TimeoutExpired:
                injector.kill()
                injector_output, injector_error = injector.communicate()
            injector_status = injector.returncode

    if finished is None:
        raise RuntimeError(
            f"Trial {index}: input response was not observed within {TIMEOUT}s "
            f"(injector status {injector_status}): output={output[-512:]!r} "
            f"injector={injector_error.decode()} trace={trace.decode()}"
        )
    points = {"start": started, "echo": finished}
    for line in injector_output.splitlines():
        parts = line.split()
        if len(parts) == 2 and parts[0] == b"captured":
            points["captured"] = int(parts[1])
    for line in trace.splitlines():
        parts = line.split()
        if len(parts) == 3 and parts[0] == b"worminal-timing":
            points[parts[1].decode()] = int(parts[2])
    phases = (PHASES if Path(env.get("WORMINAL_FRONTEND", "worminal")).name == "worminal-x11" else
              GTK_PHASES if "main" in points and not env.get("WORMINAL_LATENCY_FORWARDED") else ("echo",))
    missing = (set(phases) | {"captured"}) - points.keys()
    if missing:
        raise RuntimeError(f"Trial {index}: missing timing phases {sorted(missing)}: {trace.decode()}")
    return {phase: (points[phase] - started) / 1_000_000
            for phase in (*phases, "captured")}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--shell", action="store_true")
    parser.add_argument("--forwarded", action="store_true")
    options = parser.parse_args()
    shell = options.shell
    with isolated_display() as env:
        owner = None
        checks = ROOT / ".checks"
        checks.mkdir(exist_ok=True)
        with (checks / "latency-owner.log").open("wb") as log:
            try:
                if options.forwarded:
                    if env.get("WORMINAL_FRONTEND", "worminal") == "worminal-x11":
                        raise SystemExit("Forwarded launches require the GTK frontend")
                    title = f"Worminal latency owner {os.getpid()}"
                    owner = subprocess.Popen(
                        [str(ROOT / env.get("WORMINAL_FRONTEND", "worminal")), "-T", title, "-e", "/bin/cat"],
                        env=env, stdout=log, stderr=log)
                    subprocess.run(["xdotool", "search", "--sync", "--onlyvisible", "--name", f"^{title}$"],
                                   env=env, check=True, stdout=subprocess.DEVNULL, timeout=TIMEOUT)
                    env["WORMINAL_LATENCY_FORWARDED"] = "1"
                samples = [measure_one(env, index, shell=shell) for index in range(TRIALS)]
            finally:
                if owner:
                    owner.terminate()
                    try:
                        owner.communicate(timeout=TIMEOUT)
                    except subprocess.TimeoutExpired:
                        owner.kill()
                        owner.communicate(timeout=TIMEOUT)
    mode = "forwarded" if options.forwarded else "fresh-frontend"
    path = checks / f"latency-{mode}{'-shell' if shell else ''}.json"
    path.write_text(json.dumps({"mode": mode, "frontend": env.get("WORMINAL_FRONTEND", "worminal"),
                                "trials": TRIALS, "samples_ms": samples}, indent=2) + "\n")
    for phase, label in (("captured", "key queued by X"),
                         ("echo", "shell input read" if shell else "first key echo")):
        ordered = sorted(sample[phase] for sample in samples)
        p95 = ordered[(95 * TRIALS + 99) // 100 - 1]
        print(f"Launch to {label}: median {statistics.median(ordered):.1f} ms, "
              f"p95 {p95:.1f} ms (n={TRIALS}, private Xvfb)")
    previous = "start"
    for phase in PHASES if "display" in samples[0] else GTK_PHASES if "main" in samples[0] else ():
        duration = statistics.median(sample[phase] - sample.get(previous, 0) for sample in samples)
        print(f"  {previous} -> {phase}: {duration:.1f} ms")
        previous = phase
    print(f"Measurements: {path.relative_to(ROOT)} ({mode})")


if __name__ == "__main__":
    main()
