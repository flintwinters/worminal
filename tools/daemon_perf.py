"""Release daemon microbenchmarks with revision and raw samples retained under build/checks."""
import json
from pathlib import Path
import statistics
import subprocess

from tools import build
from tools.paths import CHECKS


def measure(label, trials):
    if not label or Path(label).name != label or label in (".", ".."):
        raise ValueError("Choose a single filename component as the measurement label")
    build.generate_theme()
    build.Builder().target("build/libworminal_engine.a")
    command = ["cargo", "test", "--locked", "--release", "--quiet", "-p", "worminal-daemon",
               "--lib", "perf_tests::hot_paths", "--", "--ignored", "--nocapture", "--test-threads=1"]
    samples = {}
    for _ in range(trials):
        result = subprocess.check_output(command, cwd=build.ROOT, text=True)
        found = 0
        for line in result.splitlines():
            if line.startswith("DAEMON_PERF "):
                _, name, elapsed, iterations = line.split()
                samples.setdefault(name, []).append(int(elapsed) / int(iterations))
                found += 1
        if not found:
            raise RuntimeError("Daemon benchmarks produced no measurements")
    report = {"revision": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
              "working_tree": subprocess.check_output(["git", "diff", "--stat"], text=True),
              "trials": trials, "unit": "nanoseconds per iteration", "samples": samples,
              "medians": {name: statistics.median(values) for name, values in samples.items()},
              "notes": "Release microbenchmarks; excludes GUI, SSH and physical presentation. "
                       "Catalog iterations assemble 64 cached tab entries; frames are full 100x40 viewports."}
    destination = CHECKS / f"daemon-perf-{label}.json"
    destination.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report["medians"], indent=2))
    print(f"Saved {destination.relative_to(build.ROOT)}")
