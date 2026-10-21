"""Observe X server pixels after real key events; optional diagnostic startup traces.

Private Xvfb measures software presentation, not compositor/display scanout or
physical keyboard latency. The fixture uses a raw PTY, so every visible glyph
must travel through keyboard dispatch, daemon input/output, and GTK rendering.
"""

import argparse
from contextlib import ExitStack
import json
import os
from pathlib import Path
import statistics
import shutil
import subprocess
import sys
import time

from Xlib import X, XK, display, error
from Xlib.ext import xtest
from rich.console import Console
from rich.table import Table

from x11 import close_window, isolated_display
from perf_metrics import descendants, memory

ROOT = Path(__file__).resolve().parent.parent
TIMEOUT = 10


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
        try:
            if window.get_wm_name() == title and window.get_attributes().map_state == X.IsViewable:
                return window
        except error.BadWindow:
            continue  # A previous launch can finish closing after query_tree.
    return None


def pixels(window):
    size = window.get_geometry()
    return window.get_image(size.width // 4, size.height // 4,
                            size.width // 2, size.height // 2, X.ZPixmap, 0xffffffff).data


def ready(window, color=0x112233):
    size = window.get_geometry()
    data = window.get_image(size.width // 2, size.height // 2, 1, 1, X.ZPixmap, 0xffffffff).data
    return int.from_bytes(data, sys.byteorder) & 0xffffff == color


def capture_failure(window):
    try:
        from PIL import Image
        size = window.get_geometry()
        capture = window.get_image(0, 0, size.width, size.height, X.ZPixmap, 0xffffffff)
        Image.frombytes("RGB", (size.width, size.height), capture.data, "raw", "BGRX").save(
            ROOT / ".checks/visible-latency-failure.png")
    except Exception:
        pass  # Preserve the original failure even if its window has disappeared.


def launch(env, connection, title, log, color=0x112233, history=False, master=None):
    started = time.monotonic_ns()
    process = subprocess.Popen([env.get("WORMINAL_FRONTEND", str(ROOT / "worminal")), "-g", "80x24", "-T", title,
                                *(["--master", master] if master else []),
                                "-e", str(ROOT / "build/latency_echo"), "--color", f"{color:06x}",
                                *(["--history"] if history else [])],
                               env=env, stdout=log, stderr=log)
    window = None
    try:
        window = wait_for(lambda: find_window(connection, title))
        mapped = time.monotonic_ns()
        wait_for(lambda: ready(window, color))
        visible = time.monotonic_ns()
        return process, window, {"map": (mapped - started) / 1e6, "ready": (visible - started) / 1e6}
    except BaseException as error:
        if window:
            capture_failure(window)
        stop(process)
        if not isinstance(error, Exception):
            raise
        raise RuntimeError(
            f"Launch failed: {title}, executable={process.args[0]}, status={process.returncode}") from error


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
    def press(symbol):
        key = connection.keysym_to_keycode(symbol)
        xtest.fake_input(connection, X.KeyPress, key)
        xtest.fake_input(connection, X.KeyRelease, key)
        connection.flush()

    # Font-driven geometry can still settle after the ready canvas first
    # appears. Calibrate exact expected images outside measured keystrokes;
    # a resize or unrelated repaint must not count as an echo response.
    expected = {}
    time.sleep(.05)
    for symbol in (XK.XK_m, XK.XK_W):
        press(symbol)
        time.sleep(.05)
        expected[symbol] = pixels(window)
    if expected[XK.XK_m] == expected[XK.XK_W]:
        capture_failure(window)
        raise RuntimeError(f"Distinct calibration glyphs missing in {window.get_wm_name()!r}")
    samples = []
    for index in range(count):
        symbol = XK.XK_W if index % 2 else XK.XK_m
        started = time.monotonic_ns()
        press(symbol)
        try:
            wait_for(lambda: pixels(window) == expected[symbol])
        except TimeoutError as error:
            capture_failure(window)
            raise RuntimeError(f"Typing failed at key {index}, window={window.get_wm_name()!r}") from error
        samples.append((time.monotonic_ns() - started) / 1e6)
    return samples


def sample_round(env, connection, label, trial, count, log, master=None):
    title = f"Latency {label} fresh {os.getpid()} {trial}"
    trace_offset = os.fstat(log.fileno()).st_size
    owner, window, timing = launch(env, connection, title, log, master=master)
    result = {f"fresh_{phase}": [value] for phase, value in timing.items()}
    try:
        if env.get("WORMINAL_TRACE_TIMING"):
            data = os.pread(log.fileno(), os.fstat(log.fileno()).st_size - trace_offset, trace_offset)
            events = {}
            for line in data.splitlines():
                parts = line.split()
                if len(parts) == 3 and parts[0] == b"worminal-timing":
                    events.setdefault(parts[1].decode(), int(parts[2]))
            if "main" not in events or "map-request" not in events:
                raise RuntimeError("Fresh startup trace is missing launcher or map events")
            result.update({f"trace_{name}_from_main": [(value - events["main"]) / 1e6]
                           for name, value in events.items() if name != "main"})
        result["typing"] = typing(connection, window, count)
        resources = memory(descendants(owner.pid))
        title = f"Latency {label} additional {os.getpid()} {trial}"
        child, _, timing = launch(env, connection, title, log, master=master)
        try:
            result.update({f"additional_{phase}": [value] for phase, value in timing.items()})
        finally:
            close_window(env, title)
            stop(child)
    finally:
        stop(owner)
    return result, resources


def summarize(samples):
    summaries = {}
    for name, values in samples.items():
        ordered = sorted(values)
        summaries[name] = {"n": len(values), "median": statistics.median(values),
                           "p95": ordered[(95 * len(values) + 99) // 100 - 1], "max": max(values)}
    return summaries


def seed_tabs(env, connection, count, log, master=None):
    """Leave real service-owned tabs behind without a running GTK owner."""
    owner = None
    try:
        for index in range(count):
            title = f"Latency saved {os.getpid()} {index}"
            process, _, _ = launch(env, connection, title, log, master=master)
            if owner is None:
                owner = process
            else:
                close_window(env, title)
                stop(process)
    finally:
        if owner:
            stop(owner)


def snapshot(directory):
    directory.mkdir(parents=True, exist_ok=False)
    for name in ("worminal", "worminald", "build/frontend.so", "build/browser.so"):
        destination = directory / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / name, destination)
    (directory / "revision").write_text(subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True))
    Console().print(f"Saved baseline: {directory}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--trials", type=int, default=10)
    parser.add_argument("--keys", type=int, default=50)
    parser.add_argument("--saved-tabs", type=int, default=0)
    parser.add_argument("--trace-startup", action="store_true")
    parser.add_argument("--bridge-delay-ms", type=int)
    parser.add_argument("--snapshot", type=Path)
    parser.add_argument("--compare", type=Path)
    args = parser.parse_args()
    if args.trials < 1 or args.keys < 1 or args.saved_tabs < 0:
        parser.error("--trials and --keys must be positive; --saved-tabs must be nonnegative")
    if args.bridge_delay_ms is not None and args.bridge_delay_ms < 0:
        parser.error("--bridge-delay-ms must be nonnegative")
    master = "preview-test" if args.bridge_delay_ms is not None else None
    if args.snapshot:
        snapshot(args.snapshot.resolve())
        return
    variants = {"candidate": ROOT / "worminal"}
    if args.compare:
        baseline = args.compare.resolve() / "worminal"
        if not baseline.is_file():
            parser.error("--compare must name a directory saved with --snapshot")
        variants["baseline"] = baseline
    checks = ROOT / ".checks"
    checks.mkdir(exist_ok=True)
    samples = {label: {} for label in variants}
    resources = {label: [] for label in variants}
    load_start = os.getloadavg()
    with ExitStack() as stack:
        contexts = {}
        for label, executable in variants.items():
            env = stack.enter_context(isolated_display())
            env.update(GDK_BACKEND="x11", WORMINAL_FRONTEND=str(executable))
            env.pop("WORMINAL_TRACE_TIMING", None)
            if args.trace_startup:
                env["WORMINAL_TRACE_TIMING"] = "1"
            if master:
                env.update(PATH=str(ROOT / "tests/fixtures/preview-master") + os.pathsep + env["PATH"],
                           WORMINAL_TEST_DAEMON=str(executable.parent / "worminald"),
                           WORMINAL_TEST_BRIDGE_DELAY=f"{args.bridge_delay_ms / 1000:.3f}")
            connection = display.Display(env["DISPLAY"])
            stack.callback(connection.close)
            log_name = "visible-latency.log" if len(variants) == 1 else f"visible-latency-{label}.log"
            log = stack.enter_context((checks / log_name).open("w+b"))
            contexts[label] = env, connection, log
        for env, connection, log in contexts.values():
            seed_tabs(env, connection, args.saved_tabs, log, master=master)
        for trial in range(args.trials):
            # Alternate which version runs first to reduce scheduling/cache bias.
            order = list(variants)[::1 if trial % 2 == 0 else -1]
            for label in order:
                env, connection, log = contexts[label]
                result, usage = sample_round(env, connection, label, trial, args.keys, log, master=master)
                if master and usage["processes"] < 2:
                    raise RuntimeError("SSH bridge process was omitted from frontend resource accounting")
                resources[label].append(usage)
                for name, values in result.items():
                    samples[label].setdefault(name, []).extend(values)
    report = {"backend": "private Xvfb, 1024x768x24", "poll_interval_ms": 1,
              "revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
              "cpu_count": os.cpu_count(), "saved_tabs": args.saved_tabs,
              "transport": "private SSH bridge fixture" if master else "local",
              "bridge_delay_ms": args.bridge_delay_ms, "trace_startup": args.trace_startup,
              "frontend_memory_samples": resources,
              "load_start": load_start, "load_end": os.getloadavg(),
              "notes": "Fresh frontend with warm filesystem caches; "
                       "seeding or the first trial starts the private daemon. "
                       "Later trials retain earlier tabs and reuse that daemon. "
                       "Ready means native echo-fixture pixels. "
                       "Typing uses calibrated expected glyph images after unmeasured warm-up keys; "
                       "includes XTest dispatch, PTY round trip, GTK paint, and XGetImage observation. "
                       "Excludes physical keyboard, desktop compositor, and monitor scanout."}
    console = Console()
    summaries = {label: summarize(values) for label, values in samples.items()}
    table = Table(title="Visible latency on private Xvfb (milliseconds)")
    for column in ("Version", "Measurement", "n", "Median", "p95", "Max"):
        table.add_column(column)
    for label, values in summaries.items():
        for name, summary in values.items():
            table.add_row(label, name, str(summary["n"]),
                          *(f"{summary[key]:.2f}" for key in ("median", "p95", "max")))
    if args.compare:
        report["variants"] = {label: {"samples_ms": values, "summary_ms": summaries[label]}
                              for label, values in samples.items()}
        report["baseline_revision"] = (args.compare / "revision").read_text().strip()
        differences = {name: [a - b for a, b in zip(values, samples["baseline"][name])]
                       for name, values in samples["candidate"].items()
                       if name != "typing" and name in samples["baseline"]}
        report["paired_difference_ms"] = differences
        report["paired_summary_ms"] = summarize(differences)
        path = checks / "visible-latency-comparison.json"
    else:
        report.update(samples_ms=samples["candidate"], summary_ms=summaries["candidate"])
        path = checks / "visible-latency.json"
    path.write_text(json.dumps(report, indent=2) + "\n")
    console.print(table)
    for label, values in resources.items():
        console.print(f"{label} frontend PSS median: "
                      f"{statistics.median(value['pss_kib'] for value in values):.0f} KiB "
                      "(after typing, before additional window)")
    if args.compare:
        console.print("Paired median differences (candidate minus baseline; negative is faster):")
        for name, summary in report["paired_summary_ms"].items():
            console.print(f"  {name}: {summary['median']:+.2f} ms")
    console.print(report["notes"])
    if args.trace_startup:
        console.print("Diagnostic traces are cumulative from launcher main, include logging overhead, "
                      "and do not replace visible-pixel observations.")
    if master:
        console.print(f"Private SSH bridge fixture, {args.bridge_delay_ms} ms requested startup delay; no remote host.")
    console.print(f"Raw measurements: {path.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
