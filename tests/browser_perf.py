"""Measure real PTY browser conversion through release modules to X server pixels."""
import argparse
from contextlib import ExitStack, contextmanager
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

from perf_metrics import descendants, memory, startup_usage
from browser_profile import collect as collect_cpu
from preview import profile
from visible_latency import capture_failure, find_window, ready, snapshot, stop, summarize, wait_for
from window_perf import keypress, window_for_pid
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


def content_visible(window, target):
    color = 0x345678 if target else 0x141414
    if not ready(window, color):
        return False
    if target:
        return True
    # WebKit can paint its configured background before the directory document.
    # Require document ink below the tab strip rather than timing a blank view.
    size = window.get_geometry()
    height = min(160, size.height - 60)
    if height <= 0:
        return False
    data = window.get_image(16, 60, min(512, size.width - 32), height,
                            X.ZPixmap, 0xffffffff).data
    return len(data) // 4 - data.count(color.to_bytes(3, "little")) > 32


def sample(executable, target, log, cold_open=False, trace_startup=False, stored_profile=None, profile_cpu=False):
    with isolated_display() as private, profile(private) as env:
        if stored_profile:
            for key in ("XDG_DATA_HOME", "XDG_CACHE_HOME"):
                env[key] = stored_profile[key]
        env.update(GDK_BACKEND="x11", GIO_USE_VFS="local", GSETTINGS_BACKEND="memory")
        if trace_startup:
            env["WORMINAL_TRACE_TIMING"] = "1"
        else:
            env.pop("WORMINAL_TRACE_TIMING", None)
        env.pop("DBUS_SESSION_BUS_ADDRESS", None)
        if profile_cpu:
            cpu_directory = Path(log.name).with_suffix(".cpu")
            cpu_directory.mkdir(exist_ok=True)
            env["WORMINAL_BROWSER_CPU_DIR"] = str(cpu_directory)
            preload = str(ROOT / "build/browser_cpu.so")
            env["LD_PRELOAD"] = preload + (":" + env["LD_PRELOAD"] if env.get("LD_PRELOAD") else "")
        connection = display.Display(env["DISPLAY"])
        owner = None
        values, resources = {}, {}
        try:
            for index, phase in enumerate(("cold",) if cold_open else ("first", "additional")):
                title = f"Browser perf terminal {index}"
                command = [str(ROOT / "build/latency_echo"), "--exec", str(executable), "--new-browser-tab",
                           *([target] if target else [])]
                launch = ([str(executable), "-T", title, "--surf",
                           target or "worminal://" + ROOT.as_uri()[7:]] if cold_open else
                          [str(executable), "-T", title, "-e", *command])
                started = time.monotonic_ns()
                process = subprocess.Popen(launch,
                                           env=env, stdout=log, stderr=log)
                if owner is None:
                    owner = process
                else:
                    process.wait(timeout=10)
                window = wait_for(lambda: window_for_pid(connection.screen().root,
                    connection.intern_atom("_NET_WM_PID"), process.pid, mapped=True)) if cold_open else wait_for(
                    lambda: find_window(connection, title))
                if not cold_open:
                    wait_for(lambda: ready(window))
                window.set_input_focus(X.RevertToParent, X.CurrentTime)
                connection.sync()
                # Terminal-only owners must continue to defer WebKit entirely.
                if not cold_open and index == 0 and "libwebkit" in Path(f"/proc/{owner.pid}/maps").read_text():
                    raise AssertionError("Terminal-only launch loaded WebKit")
                if not cold_open:
                    started = time.monotonic_ns()
                    keypress(connection, XK.XK_Return)
                try:
                    wait_for(lambda: content_visible(window, target))
                except TimeoutError:
                    capture_failure(window)
                    raise
                visible = time.monotonic_ns()
                values[phase + "_visible"] = (visible - started) / 1e6
                startup = startup_usage(owner.pid) if cold_open else None
                if startup:
                    for item in startup["processes"]:
                        try:
                            item["name"] = Path(f"/proc/{item['pid']}/comm").read_text().strip()
                        except (FileNotFoundError, ProcessLookupError):
                            item["name"] = "exited"
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
                if startup:
                    resources[phase]["startup"] = startup
                if profile_cpu:
                    resources[phase]["cpu_profile"] = collect_cpu(descendants(owner.pid), cpu_directory, visible)
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
    parser.add_argument("--cold-open", action="store_true")
    parser.add_argument("--established-profile", action="store_true")
    parser.add_argument("--trace-startup", action="store_true")
    parser.add_argument("--profile-cpu", action="store_true")
    parser.add_argument("--kind", choices=("all", "http", "file", "explorer"), default="all")
    args = parser.parse_args()
    if args.trials < 1:
        parser.error("--trials must be positive")
    if args.profile_cpu and not args.cold_open:
        parser.error("--profile-cpu requires --cold-open")
    if args.snapshot:
        snapshot(args.snapshot)
    variants = {"candidate": ROOT / "worminal"}
    if args.compare:
        variants["baseline"] = args.compare.resolve() / "worminal"
    samples = {name: {} for name in variants}
    resources = {name: [] for name in variants}
    checks = ROOT / ".checks"
    checks.mkdir(exist_ok=True)
    with canvas() as url, file_canvas(checks) as file_url, ExitStack() as profiles:
        targets = [(kind, target) for kind, target in (("http", url), ("file", file_url), ("explorer", None))
                   if args.kind == "all" or args.kind == kind]
        stored_profiles = {}
        if args.established_profile:
            for name, executable in variants.items():
                for kind, target in targets:
                    stored = profiles.enter_context(profile({}))
                    with (checks / f"browser-perf-seed-{name}-{kind}.log").open("wb") as log:
                        sample(executable, target, log, True, args.trace_startup, stored)
                    stored_profiles[name, kind] = stored
                    # sample exits its private display, waits for WebKit helpers,
                    # and stops the workspace service before any measured launch.
        for trial in range(args.trials):
            for name in list(variants)[::1 if trial % 2 == 0 else -1]:
                with (checks / f"browser-perf-{name}-{trial}.log").open("wb") as log:
                    for kind, target in targets:
                        values, usage = sample(variants[name], target, log, args.cold_open, args.trace_startup,
                                               stored_profiles.get((name, kind)), args.profile_cpu)
                        resources[name].append({"kind": kind, **usage})
                        for phase, value in values.items():
                            samples[name].setdefault(kind + "_" + phase, []).append(value)
                Console().print(f"Finished {name} trial {trial + 1}/{args.trials}")
    report = {"revision": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
              "trials": args.trials, "resources": resources,
              "cold_open": args.cold_open,
              "trace_startup": args.trace_startup,
              "established_profile": args.established_profile,
              "profile_cpu": args.profile_cpu,
              "variants": {name: {"samples_ms": values,
                                  "summary_ms": summarize(values)}
                           for name, values in samples.items()},
              "notes": ("Private Xvfb; fresh process launch directly into the browser. " if args.cold_open else
                        "Private Xvfb; real Return execs the browser replacement command in its calling PTY. ") +
                       ("Profile storage is initialized by an unmeasured launch; application, helper and service "
                        "processes stop before measured trials. " if args.established_profile else
                        "Each target and trial uses a fresh profile. ") +
                       "Visible waits for expected page pixels; HTTP and file URLs verify "
                       "real-key JavaScript response. "
                       "Explorer uses Worminal plain HTML directory links. Warm filesystem caches. "
                       "Memory includes frontend descendants, excludes service/shells. "
                       "Excludes desktop compositing and physical display latency."}
    if args.compare:
        report["baseline_revision"] = (args.compare / "revision").read_text().strip()
    if args.profile_cpu:
        report["notes"] += " CPU sampling is diagnostic; sampler setup adds overhead to elapsed timings."
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
