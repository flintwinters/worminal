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


def sample(module, label, frames, backend, log):
    context = isolated_display if backend == "x11" else wayland_display
    prefix = ROOT / f".checks/render-{label}"
    with context() as env:
        env.update(GDK_BACKEND=backend)
        process = subprocess.Popen([str(ROOT / "build/render_probe"), str(module), str(prefix), str(frames)],
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
    parser.add_argument("--backend", choices=("x11", "wayland"), default="x11")
    args = parser.parse_args()
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
                results, usage = sample(modules[label], label, args.frames, args.backend, logs[label])
                for result in results:
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
                samples.setdefault(f"{name}_wall", []).append(result["wall_us"] / (1000 * result["frames"]))
                samples.setdefault(f"{name}_cpu", []).append(result["cpu_ns"] / (1e6 * result["frames"]))
        summaries[label] = summarize(samples)
    report = {"revision": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
              "backend": args.backend, "trials": args.trials, "frames": args.frames,
              "measurements": measurements, "summary_ms_per_frame": summaries, "resources": resources,
              "module_sha256": {label: hashlib.sha256(module.read_bytes()).hexdigest()
                                for label, module in modules.items()},
              "notes": "Actual release frontend module, GTK draw to Cairo images; 20 warm-up frames per case. "
                       "Clipped case uses a 16-pixel stripe through dense ASCII. "
                       "Alternating variant order, matched geometry and exact RGBA pixel hashes. "
                       "Process CPU uses CLOCK_PROCESS_CPUTIME_ID; PSS sampled with shaped layouts alive. "
                       "Excludes input/service delivery and compositor/display presentation."}
    if args.compare and (args.compare / "revision").is_file():
        report["baseline_revision"] = (args.compare / "revision").read_text().strip()
    path = checks / f"render-perf-{args.backend}.json"
    path.write_text(json.dumps(report, indent=2) + "\n")
    console = Console()
    table = Table(title="Release rendering (milliseconds per frame)")
    for column in ("Version", "Measurement", "n", "Median", "p95"):
        table.add_column(column)
    for label, values in summaries.items():
        for name, summary in values.items():
            table.add_row(label, name, str(summary["n"]), f"{summary['median']:.3f}", f"{summary['p95']:.3f}")
    console.print(table)
    for label, usage in resources.items():
        console.print(f"{label} frontend PSS median: {statistics.median(item['pss_kib'] for item in usage):.0f} KiB")
    console.print(report["notes"])
    console.print(f"Raw measurements: {path.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
