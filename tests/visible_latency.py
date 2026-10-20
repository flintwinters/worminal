"""Observe X server pixels after real key events; no application timing hooks.

Private Xvfb measures software presentation, not compositor/display scanout or
physical keyboard latency. The fixture uses a raw PTY, so every visible glyph
must travel through keyboard dispatch, daemon input/output, and GTK rendering.
"""

import argparse
import json
import os
from pathlib import Path
import statistics
import subprocess
import sys
import time
import tty

from Xlib import X, XK, display
from Xlib.ext import xtest
from rich.console import Console
from rich.table import Table

from x11 import close_window, isolated_display

ROOT = Path(__file__).resolve().parent.parent
TIMEOUT = 10


def echo_child():
    tty.setraw(0)
    os.write(1, b"\033[?25l\033[48;2;17;34;51m\033[2J\033[H")
    while data := os.read(0, 1):
        os.write(1, b"\033[12;40H" + data)


def wait_for(probe):
    deadline = time.monotonic() + TIMEOUT
    while time.monotonic() < deadline:
        value = probe()
        if value:
            return value
        time.sleep(.001)
    raise TimeoutError("No visible response within 10 seconds")


def find_window(connection, title):
    for window in connection.screen().root.query_tree().children:
        if window.get_wm_name() == title and window.get_attributes().map_state == X.IsViewable:
            return window
    return None


def pixels(window):
    size = window.get_geometry()
    return window.get_image(size.width // 4, size.height // 4,
                            size.width // 2, size.height // 2, X.ZPixmap, 0xffffffff).data


def ready(window):
    size = window.get_geometry()
    data = window.get_image(size.width // 2, size.height // 2, 1, 1, X.ZPixmap, 0xffffffff).data
    return int.from_bytes(data, sys.byteorder) & 0xffffff == 0x112233


def launch(env, connection, title, log):
    started = time.monotonic_ns()
    process = subprocess.Popen([str(ROOT / "worminal"), "-g", "80x24", "-T", title,
                                "-e", sys.executable, str(Path(__file__).resolve()), "--echo-child"],
                               env=env, stdout=log, stderr=log)
    try:
        window = wait_for(lambda: find_window(connection, title))
        mapped = time.monotonic_ns()
        wait_for(lambda: ready(window))
        visible = time.monotonic_ns()
        return process, window, {"map": (mapped - started) / 1e6, "ready": (visible - started) / 1e6}
    except BaseException:
        stop(process)
        raise


def stop(process):
    if process.poll() is None:
        process.terminate()
    try:
        process.wait(timeout=3)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=3)


def typing(connection, window, count):
    window.set_input_focus(X.RevertToParent, X.CurrentTime)
    connection.sync()
    samples = []
    for index in range(count):
        before = pixels(window)
        key = connection.keysym_to_keycode(XK.XK_W if index % 2 else XK.XK_m)
        started = time.monotonic_ns()
        xtest.fake_input(connection, X.KeyPress, key)
        xtest.fake_input(connection, X.KeyRelease, key)
        connection.flush()
        wait_for(lambda: pixels(window) != before)
        samples.append((time.monotonic_ns() - started) / 1e6)
    return samples


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--trials", type=int, default=10)
    parser.add_argument("--keys", type=int, default=50)
    parser.add_argument("--echo-child", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.echo_child:
        echo_child()
        return
    if args.trials < 1 or args.keys < 1:
        parser.error("--trials and --keys must be positive")
    checks = ROOT / ".checks"
    checks.mkdir(exist_ok=True)
    samples = {name: [] for name in ("fresh_map", "fresh_ready", "additional_map", "additional_ready", "typing")}
    with isolated_display() as env, (checks / "visible-latency.log").open("wb") as log:
        connection = display.Display(env["DISPLAY"])
        try:
            for trial in range(args.trials):
                title = f"Latency fresh {os.getpid()} {trial}"
                owner, window, timing = launch(env, connection, title, log)
                try:
                    for phase, value in timing.items():
                        samples[f"fresh_{phase}"].append(value)
                    samples["typing"].extend(typing(connection, window, args.keys))
                    title = f"Latency additional {os.getpid()} {trial}"
                    child, _, timing = launch(env, connection, title, log)
                    try:
                        for phase, value in timing.items():
                            samples[f"additional_{phase}"].append(value)
                    finally:
                        close_window(env, title)
                        stop(child)
                finally:
                    stop(owner)
        finally:
            connection.close()
    summaries = {}
    for name, values in samples.items():
        ordered = sorted(values)
        summaries[name] = {"n": len(values), "median": statistics.median(values),
                           "p95": ordered[(95 * len(values) + 99) // 100 - 1], "max": max(values)}
    report = {"backend": "private Xvfb, 1024x768x24", "poll_interval_ms": 1,
              "revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
              "notes": "Fresh frontend with warm filesystem caches; first trial starts the private daemon. "
                       "Later trials reuse that daemon. Ready means fixture pixels visible. "
                       "Typing includes XTest dispatch, PTY round trip, GTK paint, and XGetImage observation. "
                       "Excludes physical keyboard, desktop compositor, and monitor scanout.",
              "samples_ms": samples, "summary_ms": summaries}
    path = checks / "visible-latency.json"
    path.write_text(json.dumps(report, indent=2) + "\n")
    table = Table(title="Visible latency on private Xvfb (milliseconds)")
    for column in ("Measurement", "n", "Median", "p95", "Max"):
        table.add_column(column)
    for name, summary in summaries.items():
        table.add_row(name, str(summary["n"]), *(f"{summary[key]:.2f}" for key in ("median", "p95", "max")))
    console = Console()
    console.print(table)
    console.print(report["notes"])
    console.print(f"Raw measurements: {path.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
