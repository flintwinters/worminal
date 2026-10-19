"""Compare original, terminal-only preview, and mixed preview on a private display."""

import json
import os
from pathlib import Path
import select
import statistics
import subprocess
import time

from latency import measure_one
from preview import fixture, profile
from x11 import isolated_display

ROOT = Path(__file__).resolve().parent.parent


def descendants(pid):
    result = {pid}
    try:
        children = Path(f"/proc/{pid}/task/{pid}/children").read_text().split()
    except FileNotFoundError:
        return result
    for child in children:
        result.update(descendants(int(child)))
    return result


def memory(pids):
    values = {"rss_kib": 0, "pss_kib": 0, "processes": 0}
    for pid in sorted(pids):
        try:
            lines = Path(f"/proc/{pid}/smaps_rollup").read_text().splitlines()
        except (FileNotFoundError, ProcessLookupError):
            continue
        values["processes"] += 1
        for line in lines:
            key, *parts = line.split()
            if key == "Rss:":
                values["rss_kib"] += int(parts[0])
            elif key == "Pss:":
                values["pss_kib"] += int(parts[0])
    return values


def read_until(stream, marker, timeout=10):
    output = b""
    deadline = time.monotonic() + timeout
    while marker not in output and time.monotonic() < deadline:
        ready, _, _ = select.select([stream], [], [], max(0, deadline - time.monotonic()))
        if ready:
            chunk = os.read(stream.fileno(), 65536)
            if not chunk:
                break
            output += chunk
    if marker not in output:
        raise RuntimeError(f"Missing terminal response: {marker!r}, received {output[-100:]!r}")
    return output


def key(env, *keys):
    subprocess.run(["xdotool", "key", "--delay", "0", "--clearmodifiers", *keys], env=env, check=True,
                   stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)


def steady(frontend, mixed):
    preview = frontend == "worminal-preview"
    daemon_name = "worminald-preview" if preview else "worminald"
    with isolated_display() as display, profile(display) as env, fixture() as url:
        env["GDK_BACKEND"] = "x11"
        daemon = subprocess.Popen([str(ROOT / daemon_name), "--serve"], env=env,
                                  stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        terminal = None
        try:
            title = f"Worminal perf {os.getpid()}"
            command = "stty raw -echo; printf READY; exec cat"
            started = time.monotonic_ns()
            terminal = subprocess.Popen([str(ROOT / frontend), "-T", title, "-o", "-", "-e", "/bin/sh", "-c", command],
                                        env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            read_until(terminal.stdout, b"READY")
            shell_ms = (time.monotonic_ns() - started) / 1e6
            window = subprocess.check_output(["xdotool", "search", "--onlyvisible", "--name", title],
                                             env=env, text=True).splitlines()[0]
            subprocess.run(["xdotool", "windowfocus", "--sync", window], env=env, check=True)
            if mixed:
                key(env, "ctrl+shift+b")
                # The URL dialog maps asynchronously; its title provides the readiness barrier.
                deadline = time.monotonic() + 5
                while True:
                    result = subprocess.run(["xdotool", "search", "--onlyvisible", "--name", "Open browser tab"],
                                            env=env, capture_output=True, text=True)
                    if result.returncode == 0:
                        break
                    if time.monotonic() > deadline:
                        raise RuntimeError("Browser URL dialog did not open")
                    time.sleep(.01)
                dialog = result.stdout.splitlines()[0]
                subprocess.run(["xdotool", "windowfocus", "--sync", dialog], env=env, check=True)
                subprocess.run(["xdotool", "type", "--clearmodifiers", url + "/page"], env=env, check=True)
                key(env, "Return")
                deadline = time.monotonic() + 10
                while subprocess.run(["xdotool", "search", "--name", "^Fixture$"], env=env,
                                     stdout=subprocess.DEVNULL).returncode:
                    if time.monotonic() > deadline:
                        raise RuntimeError("Mixed browser did not finish loading")
                    time.sleep(.02)
            if mixed:
                subprocess.run(["xdotool", "windowfocus", "--sync", window], env=env, check=True)
                key(env, "ctrl+Tab")
                deadline = time.monotonic() + 5
                while True:
                    actual = subprocess.check_output(["xdotool", "getwindowname", window], env=env, text=True).strip()
                    if actual == title:
                        break
                    if time.monotonic() > deadline:
                        raise RuntimeError("Mixed preview did not focus its terminal")
                    time.sleep(.01)
            inputs = []
            for _ in range(20):
                start = time.monotonic_ns()
                key(env, "z")
                read_until(terminal.stdout, b"z")
                inputs.append((time.monotonic_ns() - start) / 1e6)
            time.sleep(1)
            frontend_memory = memory(descendants(terminal.pid))
            service_memory = memory(descendants(daemon.pid))
            return {"shell_ready_ms": round(shell_ms, 2),
                    "input_median_ms": round(statistics.median(inputs), 2),
                    "input_p95_ms": round(sorted(inputs)[18], 2),
                    "frontend_and_helpers": frontend_memory, "service_and_shell": service_memory}
        finally:
            if terminal:
                terminal.terminate()
                terminal.communicate(timeout=5)
            subprocess.run([str(ROOT / daemon_name), "--stop"], env=env,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            daemon.communicate(timeout=5)


def main():
    result = {"environment": "private Xvfb; warm service; 20 launch/input trials; helper-inclusive RSS/PSS"}
    for frontend in ("worminal", "worminal-preview"):
        launch = {}
        with isolated_display() as env:
            env["WORMINAL_FRONTEND"] = frontend
            env["GDK_BACKEND"] = "x11"
            for shell in (False, True):
                samples = [measure_one(env, index, shell) for index in range(20)]
                values = sorted(sample["echo"] for sample in samples)
                launch["shell" if shell else "cat"] = {"median_ms": round(statistics.median(values), 2),
                                                       "p95_ms": round(values[18], 2)}
        result[frontend] = {"launch": launch, "terminal": steady(frontend, False)}
        if frontend == "worminal-preview":
            result[frontend]["mixed"] = steady(frontend, True)
        print(f"{frontend}: launch {launch['cat']['median_ms']} ms; shell {launch['shell']['median_ms']} ms")
    path = ROOT / ".checks/preview-performance.json"
    path.write_text(json.dumps(result, indent=2) + "\n")
    for name in ("worminal", "worminal-preview"):
        for mode in ("terminal", "mixed"):
            if mode not in result[name]:
                continue
            trial = result[name][mode]
            client, service = trial["frontend_and_helpers"], trial["service_and_shell"]
            print(f"{name} {mode}: input {trial['input_median_ms']} ms; "
                  f"frontend/helpers RSS {client['rss_kib'] / 1024:.1f} MiB, PSS {client['pss_kib'] / 1024:.1f} MiB "
                  f"({client['processes']} processes); service/shell RSS {service['rss_kib'] / 1024:.1f} MiB")
    print(f"Full measurements: {path.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
