"""Release daemon microbenchmarks with revision and raw samples retained under build/checks."""
from contextlib import ExitStack
import json
from pathlib import Path
import shutil
import statistics
import subprocess
import sys
import tempfile

from tools import build
from tools.paths import CHECKS

COMMAND = ["cargo", "test", "--locked", "--release", "--quiet", "-p", "worminal-daemon",
           "--lib", "perf_tests::hot_paths", "--", "--ignored", "--nocapture", "--test-threads=1"]


def sample(root):
    result = subprocess.check_output(COMMAND, cwd=root, text=True)
    values = {}
    for line in result.splitlines():
        if line.startswith("DAEMON_PERF "):
            _, name, elapsed, iterations = line.split()
            values[name] = int(elapsed) / int(iterations)
    if not values:
        raise RuntimeError("Daemon benchmarks produced no measurements")
    return values


def measure(label, trials, compare=None):
    if not label or Path(label).name != label or label in (".", ".."):
        raise ValueError("Choose a single filename component as the measurement label")
    build.generate_theme()
    build.Builder().target("build/libworminal_engine.a")
    with ExitStack() as stack:
        roots = {"current": build.ROOT}
        revision = None
        if compare:
            from tools.reference import extract
            revision = subprocess.check_output(["git", "rev-parse", "--verify", compare + "^{commit}"],
                                               cwd=build.ROOT, text=True).strip()
            print(f"Preparing disposable baseline {revision[:12]}", flush=True)
            baseline = Path(stack.enter_context(tempfile.TemporaryDirectory(prefix="daemon-baseline-", dir=CHECKS)))
            extract(revision, baseline, paths=("src", "tests", "tools", "assets", "manage.py",
                                               "Cargo.toml", "Cargo.lock", ".cargo"))
            # Both owners execute the exact same workload and CPU clock, independent of their implementations.
            shutil.copyfile(build.ROOT / "src/daemon/src/perf_tests.rs", baseline / "src/daemon/src/perf_tests.rs")
            subprocess.run([sys.executable, "manage.py", "daemon-perf", "--label", "warmup", "--trials", "1"],
                           cwd=baseline, check=True, stdout=subprocess.DEVNULL)
            roots["baseline"] = baseline
        for root in roots.values():
            sample(root)  # Compile and warm both owners before alternating measured trials.
        samples = {variant: {} for variant in roots}
        for trial in range(trials):
            print(f"Measuring trial {trial + 1}/{trials}", flush=True)
            order = list(roots) if trial % 2 == 0 else list(reversed(roots))
            for variant in order:
                values = sample(roots[variant])
                if samples[variant] and values.keys() != samples[variant].keys():
                    raise RuntimeError("Daemon workloads changed during measurement")
                for name, value in values.items():
                    samples[variant].setdefault(name, []).append(value)
    medians = {variant: {name: statistics.median(values) for name, values in group.items()}
               for variant, group in samples.items()}
    report = {"revision": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
              "baseline_revision": revision,
              "working_tree": subprocess.check_output(["git", "diff", "--stat"], text=True),
              "trials": trials, "unit": "thread CPU nanoseconds per iteration", "samples": samples,
              "medians": medians,
              "notes": "Release microbenchmarks; excludes GUI, SSH and physical presentation. "
                       "Catalog iterations assemble 64 cached tab entries; frames are full 100x40 viewports. "
                       "Comparison uses identical current workloads, warmed builds and alternating owner order."}
    destination = CHECKS / f"daemon-perf-{label}.json"
    destination.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report["medians"], indent=2))
    print(f"Saved {destination.relative_to(build.ROOT)}")
