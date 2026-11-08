"""Release daemon microbenchmarks with revision and raw samples retained under build/checks."""
from contextlib import ExitStack
import hashlib
import json
import os
from pathlib import Path
import shutil
import statistics
import subprocess
import sys
import tempfile
import zlib

from tools import build
from tools.paths import CHECKS

COMMAND = ["cargo", "test", "--locked", "--release", "--quiet", "-p", "worminal-daemon",
           "--lib", "perf_tests::hot_paths", "--", "--ignored", "--nocapture", "--test-threads=1"]


def compressed(path):
    """Estimate OpenSSH -C wire payload: one zlib level-6 stream, synced per recorded chunk."""
    data, stream, total, offset = path.read_bytes(), zlib.compressobj(6), 0, 0
    while offset < len(data):
        length = int.from_bytes(data[offset:offset + 4], "big")
        chunk = data[offset + 4:offset + 4 + length]
        offset += 4 + length
        total += len(stream.compress(chunk)) + len(stream.flush(zlib.Z_SYNC_FLUSH))
    return total


def sample(root, network=False):
    command = [part.replace("perf_tests::hot_paths", "service::network_tests::transport_model")
               for part in COMMAND] if network else COMMAND
    with tempfile.TemporaryDirectory(prefix="network-streams-", dir=CHECKS) as streams:
        result = subprocess.check_output(command, cwd=root, text=True,
                                         env=os.environ | {"WORMINAL_NETWORK_STREAMS": streams})
        values = {f"{path.name}_zlib_bytes": compressed(path) for path in Path(streams).iterdir()}
    for line in result.splitlines():
        if network and line.startswith("NETWORK_PERF "):
            _, name, value = line.split()
            values[name] = int(value)
        elif not network and line.startswith("DAEMON_PERF "):
            _, name, elapsed, iterations = line.split()
            values[name] = int(elapsed) / int(iterations)
    if not values:
        raise RuntimeError("Daemon benchmarks produced no measurements")
    return values


def measure(label, trials, compare=None, *, network=False):
    entrypoint = "network-perf" if network else "daemon-perf"
    workload = Path("src/daemon/src") / ("network_tests.rs" if network else "perf_tests.rs")
    workload_hash = hashlib.sha256((build.ROOT / workload).read_bytes()).hexdigest()
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
            relative = workload
            shutil.copyfile(build.ROOT / relative, baseline / relative)
            if network:
                service = baseline / "src/daemon/src/service.rs"
                source = service.read_text()
                if "mod network_tests;" not in source:
                    service.write_text(source + '\n#[cfg(test)]\n#[path = "network_tests.rs"]\nmod network_tests;\n')
            subprocess.run([sys.executable, "manage.py", "daemon-perf", "--label", "warmup", "--trials", "1"],
                           cwd=baseline, check=True, stdout=subprocess.DEVNULL)
            roots["baseline"] = baseline
        for root in roots.values():
            sample(root, network)  # Compile and warm both owners before alternating measured trials.
        samples = {variant: {} for variant in roots}
        for trial in range(trials):
            print(f"Measuring trial {trial + 1}/{trials}", flush=True)
            order = list(roots) if trial % 2 == 0 else list(reversed(roots))
            for variant in order:
                values = sample(roots[variant], network)
                if samples[variant] and values.keys() != samples[variant].keys():
                    raise RuntimeError("Daemon workloads changed during measurement")
                for name, value in values.items():
                    samples[variant].setdefault(name, []).append(value)
    if hashlib.sha256((build.ROOT / workload).read_bytes()).hexdigest() != workload_hash:
        raise RuntimeError("Measurement workload changed during comparison; rerun with stable sources")
    medians = {variant: {name: statistics.median(values) for name, values in group.items()}
               for variant, group in samples.items()}
    report = {"revision": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
              "baseline_revision": revision, "workload_sha256": workload_hash,
              "working_tree": subprocess.check_output(["git", "diff", "--stat"], text=True),
              "trials": trials, "unit": "metric suffix (bytes, ms, frames)" if network else
              "thread CPU nanoseconds per iteration", "samples": samples,
              "medians": medians,
              "notes": "Release microbenchmarks; excludes GUI, SSH and physical presentation. "
                       "Catalog iterations assemble 64 cached tab entries; frames are full 100x40 viewports. "
                       "Comparison uses identical current workloads, warmed builds and alternating owner order."}
    if network:
        report["notes"] = ("Deterministic service model: 120x40, 250 dense updates at virtual 16 ms intervals; "
                           "4096 raw protocol bytes drained per interval (2.048 Mbps) before compression, no "
                           "RTT, kernel/SSH buffering or GTK presentation. *_zlib_bytes estimate SSH -C payload, "
                           "excluding SSH/TCP/IP headers. Sparse: 100 counter updates. "
                           "Same workload on both revisions, capability negotiation selects supported encoding. "
                           "Frame age is virtual source-to-complete-frame age; tail is drain time after output stops.")
    destination = CHECKS / f"{entrypoint}-{label}.json"
    destination.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report["medians"], indent=2))
    print(f"Saved {destination.relative_to(build.ROOT)}")
