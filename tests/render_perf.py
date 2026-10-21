"""Compare actual release terminal rendering, pixels, CPU, and memory on private displays."""

import argparse
from contextlib import ExitStack
import hashlib
import json
from pathlib import Path
import statistics
import subprocess
import sys

from PIL import Image
from rich.console import Console
from rich.table import Table

from frontend_perf import read_until
from perf_metrics import descendants, memory
from preview import wayland_display
from visible_latency import stop, summarize
from x11 import isolated_display

ROOT = Path(__file__).resolve().parent.parent


def sample(module, label, frames, backend, log, resize=False, tabs=0):
    context = isolated_display if backend == "x11" else wayland_display
    prefix = ROOT / f".checks/render-{label}"
    with context() as env:
        env.update(GDK_BACKEND=backend)
        process = subprocess.Popen([str(ROOT / "build/render_probe"), str(module), str(prefix), str(frames),
                                    *(["--resize"] if resize else []), *(["--tabs", str(tabs)] if tabs else [])],
                                   env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log)
        try:
            try:
                results = json.loads(read_until(process.stdout, b"\n", timeout=10 + frames * .3))
            except (RuntimeError, ValueError) as error:
                raise RuntimeError(f"Render probe failed: status={process.poll()}, log={log.name}") from error
            resources = memory(descendants(process.pid))
            for result in results:
                with Image.open(f"{prefix}-{result['case']}.png") as image:
                    result["pixel_sha256"] = hashlib.sha256(image.convert("RGBA").tobytes()).hexdigest()
                    result["image_size"] = image.size
            process.communicate(input=b"\n", timeout=5)
            if process.returncode:
                raise RuntimeError(f"Render probe exited with {process.returncode}; see {log.name}")
            return results, resources
        finally:
            stop(process)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--trials", type=int, default=10)
    parser.add_argument("--frames", type=int, default=60)
    parser.add_argument("--compare", type=Path)
    parser.add_argument("--resize", action="store_true")
    parser.add_argument("--tabs", type=int, default=0)
    parser.add_argument("--backend", choices=("x11", "wayland"), default="x11")
    args = parser.parse_args()
    if args.tabs < 0 or args.tabs > 4096 or (args.tabs and args.resize):
        parser.error("--tabs must be 0..4096 and cannot be combined with --resize")
    if args.trials < 1 or args.frames < 1:
        parser.error("--trials and --frames must be positive")
    modules = {"candidate": ROOT / "build/frontend.so"}
    if args.compare:
        modules["baseline"] = args.compare.resolve() / "build/frontend.so"
        if not modules["baseline"].is_file():
            parser.error("--compare must name a latency-visible snapshot")
    checks = ROOT / ".checks"
    checks.mkdir(exist_ok=True)
    measurements = {label: [] for label in modules}
    resources = {label: [] for label in modules}
    expected = {}
    with ExitStack() as stack:
        logs = {label: stack.enter_context((checks / f"render-{label}.log").open("wb")) for label in modules}
        for trial in range(args.trials):
            for label in list(modules)[::1 if trial % 2 == 0 else -1]:
                results, usage = sample(modules[label], label, args.frames, args.backend, logs[label],
                                        args.resize, args.tabs)
                for result in results:
                    if args.resize and label == "candidate" and result["local_history_rows"]:
                        raise RuntimeError("Replica frame import accumulated local scrollback")
                    identity = result["pixel_sha256"], result["image_size"], result["cols"], result["rows"]
                    if expected.setdefault(result["case"], identity) != identity:
                        raise RuntimeError(f"Rendered pixels/geometry differ: {label} {result['case']} trial {trial}")
                measurements[label].append(results)
                resources[label].append(usage)
    summaries = {}
    for label, trials in measurements.items():
        samples = {}
        for results in trials:
            for result in results:
                name = result["case"]
                count = result["cycles"] if args.resize else result["frames"]
                samples.setdefault(f"{name}_wall", []).append(result["wall_us"] / (1000 * count))
                samples.setdefault(f"{name}_cpu", []).append(result["cpu_ns"] / (1e6 * count))
        summaries[label] = summarize(samples)
    summary_key = "summary_ms_per_resize_cycle" if args.resize else "summary_ms_per_frame"
    report = {"revision": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
              "measurement_unit": "resize cycle (two frames)" if args.resize else "paint frame",
              "tabs": args.tabs, "resize": args.resize, "backend": args.backend,
              "trials": args.trials, "frames": args.frames,
              "measurements": measurements, summary_key: summaries, "resources": resources,
              "probe_sha256": hashlib.sha256((ROOT / "build/render_probe").read_bytes()).hexdigest(),
              "module_sha256": {label: hashlib.sha256(module.read_bytes()).hexdigest()
                                for label, module in modules.items()},
              "notes": "Actual release frontend module, GTK draw to Cairo images; 20 warm-up frames per case. "
                       "Clipped case uses a 16-pixel stripe through dense ASCII. "
                       "Dense Unicode cycles Greek, Latin, Cyrillic, Hebrew, Arabic, box drawing, CJK and emoji; "
                       "wide cells have matching dummy columns. "
                       "Alternating variant order, matched geometry and exact RGBA pixel hashes. "
                       "Process CPU uses CLOCK_PROCESS_CPUTIME_ID; PSS sampled with shaped layouts alive. "
                       "Excludes input/service delivery and compositor/display presentation."}
    if args.compare and (args.compare / "revision").is_file():
        report["baseline_revision"] = (args.compare / "revision").read_text().strip()
    if args.resize:
        report["notes"] = ("Actual release frontend; each cycle imports two full, soft-wrapped owner frames "
                           "at alternating viewport widths. Twenty warm-up cycles precede measurement. "
                           "Only packet import is timed; final painting is outside the measured interval. "
                           "The final exact image and retained storage are checked. "
                           "Cell storage excludes parser buffers, fonts, row pointers and allocator overhead.")
    if args.tabs:
        report["notes"] = ("Actual release tab strip with distinct directory labels and the last tab active; "
                           "twenty warm-up frames precede timed GTK draw to Cairo images. "
                           "Exact RGBA and pixel geometry match across variants. "
                           "No terminal cells, service or shell are included.")
    path = checks / f"render-perf-{args.backend}.json"
    path.write_text(json.dumps(report, indent=2) + "\n")
    console = Console()
    table = Table(title="Release imports (milliseconds per resize cycle)" if args.resize else
                        "Release rendering (milliseconds per frame)")
    for column in ("Version", "Measurement", "n", "Median", "p95"):
        table.add_column(column)
    for label, values in summaries.items():
        for name, summary in values.items():
            table.add_row(label, name, str(summary["n"]), f"{summary['median']:.3f}", f"{summary['p95']:.3f}")
    console.print(table)
    for label, usage in resources.items():
        console.print(f"{label} frontend PSS median: {statistics.median(item['pss_kib'] for item in usage):.0f} KiB")
    if args.resize:
        for label, trials in measurements.items():
            console.print(f"{label} retained local history rows: "
                          f"{statistics.median(trial[0]['local_history_rows'] for trial in trials):.0f}; "
                          "glyph storage: "
                          f"{statistics.median(trial[0]['glyph_storage_bytes'] for trial in trials):.0f} bytes")
    console.print(report["notes"])
    console.print(f"Raw measurements: {path.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
