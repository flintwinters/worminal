"""Measure real PTY browser conversion through release modules to X server pixels."""
import argparse
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import subprocess
import tempfile
import threading
import time

from Xlib import X, XK, display
from rich.console import Console
from rich.table import Table

from perf_metrics import descendants, memory
from preview import profile
from visible_latency import capture_failure, find_window, ready, snapshot, stop, summarize, wait_for
from window_perf import keypress
from x11 import isolated_display

ROOT = Path(__file__).resolve().parent.parent
CANVAS_HTML = ("<!doctype html><title>Browser canvas</title>"
               "<style>html{background:#345678}</style>"
               "<script>onkeydown=()=>document.documentElement.style.background='#789abc'</script>")


class Canvas(BaseHTTPRequestHandler):
    def do_GET(self):
        body = CANVAS_HTML.encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args):
        pass


@contextmanager
def canvas():
    server = ThreadingHTTPServer(("127.0.0.1", 0), Canvas)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}/"
    finally:
        server.shutdown()
        server.server_close()
        worker.join()


@contextmanager
def file_canvas(checks):
    with tempfile.TemporaryDirectory(prefix="browser-canvas-", dir=checks) as directory:
        path = Path(directory) / "canvas.html"
        path.write_text(CANVAS_HTML)
        yield path.as_uri()


def sample(executable, target, log):
    with isolated_display() as private, profile(private) as env:
        env.update(GDK_BACKEND="x11", GIO_USE_VFS="local", GSETTINGS_BACKEND="memory",
                   WORMINAL_TRACE_TIMING="1")
        env.pop("DBUS_SESSION_BUS_ADDRESS", None)
        connection = display.Display(env["DISPLAY"])
        owner = None
        values, resources = {}, {}
        try:
            for index, phase in enumerate(("first", "additional")):
                title = f"Browser perf terminal {index}"
                command = [str(ROOT / "build/latency_echo"), "--exec", str(executable), "--new-browser-tab",
                           *([target] if target else [])]
                process = subprocess.Popen([str(executable), "-T", title, "-e", *command],
                                           env=env, stdout=log, stderr=log)
                if owner is None:
                    owner = process
                else:
                    process.wait(timeout=10)
                window = wait_for(lambda: find_window(connection, title))
                wait_for(lambda: ready(window))
                window.set_input_focus(X.RevertToParent, X.CurrentTime)
                connection.sync()
                # Terminal-only owners must continue to defer WebKit entirely.
                if index == 0 and "libwebkit" in Path(f"/proc/{owner.pid}/maps").read_text():
                    raise AssertionError("Terminal-only launch loaded WebKit")
                started = time.monotonic_ns()
                keypress(connection, XK.XK_Return)
                try:
                    wait_for(lambda: ready(window, 0x345678 if target else 0xffffff))
                except TimeoutError:
                    capture_failure(window)
                    raise
                values[phase + "_visible"] = (time.monotonic_ns() - started) / 1e6
                if target:
                    started = time.monotonic_ns()
                    keypress(connection, XK.XK_a)
                    wait_for(lambda: ready(window, 0x789abc))
                    values[phase + "_input"] = (time.monotonic_ns() - started) / 1e6
                else:
                    def explorer_title():
                        title = window.get_full_property(connection.intern_atom("_NET_WM_NAME"), X.AnyPropertyType)
                        return title and str(ROOT) in bytes(title.value).decode("utf-8")
                    wait_for(explorer_title)
                resources[phase] = memory(descendants(owner.pid))
            return values, resources
        finally:
            if owner:
                children = descendants(owner.pid) - {owner.pid}
                stop(owner)
                def helpers_exited():
                    for pid in children:
                        try:
                            if Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[0] != "Z":
                                return False
                        except (FileNotFoundError, ProcessLookupError):
                            pass
                    return True
                wait_for(helpers_exited)
            connection.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--trials", type=int, default=5)
    parser.add_argument("--snapshot", type=Path)
    parser.add_argument("--compare", type=Path)
    args = parser.parse_args()
    if args.trials < 1:
        parser.error("--trials must be positive")
    if args.snapshot:
        snapshot(args.snapshot)
    variants = {"candidate": ROOT / "worminal"}
    if args.compare:
        variants["baseline"] = args.compare.resolve() / "worminal"
    samples = {name: {} for name in variants}
    resources = {name: [] for name in variants}
    checks = ROOT / ".checks"
    checks.mkdir(exist_ok=True)
    with canvas() as url, file_canvas(checks) as file_url:
        for trial in range(args.trials):
            for name in list(variants)[::1 if trial % 2 == 0 else -1]:
                with (checks / f"browser-perf-{name}-{trial}.log").open("wb") as log:
                    for kind, target in (("http", url), ("file", file_url), ("explorer", None)):
                        values, usage = sample(variants[name], target, log)
                        resources[name].append({"kind": kind, **usage})
                        for phase, value in values.items():
                            samples[name].setdefault(kind + "_" + phase, []).append(value)
                Console().print(f"Finished {name} trial {trial + 1}/{args.trials}")
    report = {"revision": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
              "trials": args.trials, "resources": resources,
              "variants": {name: {"samples_ms": values,
                                  "summary_ms": summarize(values)}
                           for name, values in samples.items()},
              "notes": "Private Xvfb; real Return execs the browser replacement command in its calling PTY. "
                       "First and additional browser views use fresh profiles per target and trial. "
                       "Visible waits for expected page pixels; HTTP and file URLs verify real-key JavaScript response. "
                       "Explorer uses Worminal plain HTML directory links. Warm filesystem caches. "
                       "Memory includes frontend descendants, excludes service/shells. "
                       "Excludes desktop compositing and physical display latency."}
    if args.compare:
        report["baseline_revision"] = (args.compare / "revision").read_text().strip()
    path = checks / "browser-perf.json"
    path.write_text(json.dumps(report, indent=2) + "\n")
    table = Table(title="Browser tab latency (ms)")
    for column in ("Version", "Measurement", "n", "Median", "p95", "Max"):
        table.add_column(column)
    for name, result in report["variants"].items():
        for key, summary in result["summary_ms"].items():
            table.add_row(name, key, str(summary["n"]),
                          *(f"{summary[field]:.2f}" for field in ("median", "p95", "max")))
    Console().print(table)
    Console().print(report["notes"])
    Console().print(f"Raw measurements: {path.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
