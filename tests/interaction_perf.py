"""Measure GTK tab ownership, cached switching, and scroll pixels on private Xvfb."""

import argparse
from contextlib import ExitStack
import json
import os
from pathlib import Path
import statistics
import subprocess
import sys
import time

from Xlib import X, XK, display
from Xlib.ext import xtest
from rich.console import Console
from rich.table import Table

from perf_metrics import cpu_time, descendants, main_cpu_time, memory
from visible_latency import capture_failure, launch, ready, stop, summarize, wait_for
from window_perf import keypress
from x11 import isolated_display

ROOT = Path(__file__).resolve().parent.parent
LIVE = 0x112233
HISTORY = 0x445566
OTHER = 0x778899


def color_at(window, x, y):
    data = window.get_image(x, y, 1, 1, X.ZPixmap, 0xffffffff).data
    return int.from_bytes(data, sys.byteorder) & 0xffffff


def measure(action, visible, window):
    started = time.monotonic_ns()
    action()
    try:
        wait_for(visible)
    except TimeoutError:
        capture_failure(window)
        raise
    return (time.monotonic_ns() - started) / 1e6


def painted_since(log, offset):
    return sum(line.startswith(b"worminal-timing paint ")
               for line in Path(log.name).read_bytes()[offset:].splitlines())


def sample(executable, samples, log, pace_ms):
    with isolated_display() as env:
        env.update(GDK_BACKEND="x11", WORMINAL_FRONTEND=str(executable), WORMINAL_TRACE_TIMING="1")
        connection = display.Display(env["DISPLAY"])
        owner = child = None
        try:
            owner, window, _ = launch(env, connection, "Interaction history", log, history=True)
            child, _, _ = launch(env, connection, "Interaction second", log, color=OTHER)
            # XGetImage on an obscured window is undefined. Raise the measured
            # window and focus it; no desktop session or window manager is used.
            window.configure(stack_mode=X.Above)
            window.set_input_focus(X.RevertToParent, X.CurrentTime)
            connection.sync()
            time.sleep(.15)  # Exclude initial font/geometry settling from interactions.

            def select(index):
                keypress(connection, XK.XK_Control_L, XK.XK_1 if index == 1 else XK.XK_2)

            log.flush()
            claim_start = log.tell()
            claim_cpu = cpu_time(owner.pid)
            claim_main_cpu = main_cpu_time(owner.pid)
            results = {"tab_first_claim": [measure(lambda: select(2), lambda: ready(window, OTHER), window)]}
            time.sleep(.05)  # Include any deferred claim repaint in its accounting.
            claim_usage = {"cpu_seconds": cpu_time(owner.pid) - claim_cpu,
                           "main_cpu_seconds": main_cpu_time(owner.pid) - claim_main_cpu,
                           "paint_calls": painted_since(log, claim_start)}
            select(1)
            wait_for(lambda: ready(window, LIVE))
            for _ in range(3):
                select(2)
                wait_for(lambda: ready(window, OTHER))
                select(1)
                wait_for(lambda: ready(window, LIVE))
            started_cpu = cpu_time(owner.pid)
            log.flush()
            paint_start = log.tell()
            results["tab_cached"] = []
            for index in range(samples):
                time.sleep(pace_ms / 1000)
                tab, color = (2, OTHER) if index % 2 == 0 else (1, LIVE)
                results["tab_cached"].append(measure(lambda: select(tab), lambda: ready(window, color), window))
            select(1)
            wait_for(lambda: ready(window, LIVE))
            size = window.get_geometry()
            x = size.width // 2
            # Locate the live canvas below the tab strip independently of font
            # metrics; wheel scrolling must reveal history at its first row.
            y = next(row for row in range(size.height // 2) if color_at(window, x, row) == LIVE)
            window.warp_pointer(x, y)
            connection.sync()

            def wheel(up):
                button = 4 if up else 5
                xtest.fake_input(connection, X.ButtonPress, button)
                xtest.fake_input(connection, X.ButtonRelease, button)
                connection.flush()

            for name in ("scroll_wheel", "scroll_page"):
                results[name] = []
                for index in range(samples + samples % 2):
                    time.sleep(pace_ms / 1000)
                    up = index % 2 == 0
                    color = HISTORY if up else LIVE
                    action = (lambda: wheel(up)) if name == "scroll_wheel" else (
                        lambda: keypress(connection, XK.XK_Shift_L, XK.XK_Prior if up else XK.XK_Next))
                    elapsed = measure(action, lambda: color_at(window, x, y) == color, window)
                    if index < samples:
                        results[name].append(elapsed)
            cpu_seconds = cpu_time(owner.pid) - started_cpu
            paints = painted_since(log, paint_start)
            usage = {**memory(descendants(owner.pid)), "cpu_seconds": cpu_seconds,
                     "paint_calls": paints, "first_claim": claim_usage}
            # Rapid input is independent of observation speed. An early first
            # response must not turn every wheel event into a rendered frame.
            started_cpu = cpu_time(owner.pid)
            paint_start = log.tell()
            started = time.monotonic_ns()
            for up in (True, False):
                for _ in range(50):
                    wheel(up)
                    time.sleep(.002)
                wait_for(lambda: color_at(window, x, y) == (HISTORY if up else LIVE))
            time.sleep(.05)  # Drain the last scheduled frame before accounting.
            usage["wheel_burst"] = {
                "events": 100, "requested_interval_ms": 2, "wall_ms": (time.monotonic_ns() - started) / 1e6,
                "cpu_seconds": cpu_time(owner.pid) - started_cpu,
                "paint_calls": painted_since(log, paint_start)}
            return results, usage
        finally:
            if child:
                stop(child)
            if owner:
                stop(owner)
            connection.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--trials", type=int, default=10)
    parser.add_argument("--samples", type=int, default=20)
    parser.add_argument("--compare", type=Path)
    parser.add_argument("--pace-ms", type=float, default=0)
    args = parser.parse_args()
    if args.trials < 1 or args.samples < 1 or args.pace_ms < 0:
        parser.error("--trials and --samples must be positive; --pace-ms must be nonnegative")
    variants = {"candidate": ROOT / "worminal"}
    if args.compare:
        variants["baseline"] = args.compare.resolve() / "worminal"
        if not variants["baseline"].is_file():
            parser.error("--compare must name a latency-visible snapshot directory")
    checks = ROOT / ".checks"
    checks.mkdir(exist_ok=True)
    samples = {label: {} for label in variants}
    resources = {label: [] for label in variants}
    with ExitStack() as stack:
        logs = {label: stack.enter_context((checks / f"interaction-{label}.log").open("wb")) for label in variants}
        for trial in range(args.trials):
            for label in list(variants)[::1 if trial % 2 == 0 else -1]:
                result, usage = sample(variants[label], args.samples, logs[label], args.pace_ms)
                resources[label].append(usage)
                for name, values in result.items():
                    samples[label].setdefault(name, []).extend(values)
    report = {"revision": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
              "trials": args.trials, "samples_per_trial": args.samples, "pace_ms": args.pace_ms,
              "resources": resources,
              "variants": {label: {"samples_ms": values, "summary_ms": summarize(values)}
                           for label, values in samples.items()},
              "notes": "Private Xvfb, warm filesystem caches, one fresh workspace per trial. "
                       "First claim includes view ownership transfer; cached switching follows three warm-up pairs. "
                       "Wheel/page scrolling waits for expected history/live pixels at the first canvas row. "
                       "Pacing delays precede requests and are excluded from their latency. Paint tracing is enabled. "
                       "CPU uses process ticks plus nanosecond scheduled runtime for the claim's main thread. "
                       "Excludes desktop compositing, physical input, and display scanout."}
    if args.compare:
        report["baseline_revision"] = (args.compare / "revision").read_text().strip()
    path = checks / "interaction-perf.json"
    path.write_text(json.dumps(report, indent=2) + "\n")
    table = Table(title="Visible GTK interactions (milliseconds)")
    for column in ("Version", "Measurement", "n", "Median", "p95", "Max"):
        table.add_column(column)
    for label, values in report["variants"].items():
        for name, summary in values["summary_ms"].items():
            table.add_row(label, name, str(summary["n"]),
                          *(f"{summary[key]:.2f}" for key in ("median", "p95", "max")))
    console = Console()
    console.print(table)
    for label, values in resources.items():
        console.print(f"{label} median paints/CPU per trial: "
                      f"{statistics.median(value['paint_calls'] for value in values):.0f} / "
                      f"{1000 * statistics.median(value['cpu_seconds'] for value in values):.0f} ms; "
                      "100-event wheel burst: "
                      f"{statistics.median(value['wheel_burst']['paint_calls'] for value in values):.0f} / "
                      f"{1000 * statistics.median(value['wheel_burst']['cpu_seconds'] for value in values):.0f} ms")
        console.print("  first claim: "
                      f"{statistics.median(value['first_claim']['paint_calls'] for value in values):.0f} paints / "
                      f"{1000 * statistics.median(value['first_claim']['cpu_seconds'] for value in values):.0f} "
                      "ms CPU; "
                      f"{1000 * statistics.median(value['first_claim']['main_cpu_seconds'] for value in values):.2f} "
                      "ms main-thread runtime")
    console.print(report["notes"])
    console.print(f"Raw measurements: {path.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
