"""Wall-clock spans for build, check and update runs, and the report that makes them actionable.

Every `manage.py` invocation opens a run file `build/checks/timings/<stamp>-<command>.jsonl`. Spans nest: the
run file and the innermost open span travel to child processes through the environment, so a build step, a test
module or a nested `manage.py` all attach to the span that started them. Child processes append single JSON
lines, which stay whole under concurrent writers. Report with `manage.py timings`.
"""
import contextlib
import itertools
import json
import os
from pathlib import Path
import threading
import time

ROOT = Path(__file__).resolve().parent.parent
DIRECTORY = ROOT / "build" / "checks" / "timings"
FILE_VARIABLE = "WORMINAL_TIMING_FILE"
PARENT_VARIABLE = "WORMINAL_TIMING_PARENT"

_counter = itertools.count(1)
_local = threading.local()
_main = []  # open spans of the thread that opened the run; worker threads without their own attach here


def _stack():
    if not hasattr(_local, "stack"):
        _local.stack = []
    return _local.stack


def active():
    return os.environ.get(FILE_VARIABLE)


def current():
    """Identity of the innermost open span for this thread, for child processes started from it."""
    return (_stack() or _main or [os.environ.get(PARENT_VARIABLE, "")])[-1]


def start(command):
    """Open the run file for this process tree unless a parent already did."""
    if active():
        return
    DIRECTORY.mkdir(parents=True, exist_ok=True)
    stamp = time.strftime("%Y%m%d-%H%M%S")
    os.environ[FILE_VARIABLE] = str(DIRECTORY / f"{stamp}-{command or 'manage'}.jsonl")


@contextlib.contextmanager
def span(name, kind="step"):
    """Record how long the block takes, nested under the innermost open span."""
    path = active()
    if not path:
        yield
        return
    stack = _stack()
    parent = current()
    identity = f"{os.getpid()}.{next(_counter)}"
    stack.append(identity)
    main = threading.current_thread() is threading.main_thread()
    if main:
        _main.append(identity)
    previous = os.environ.get(PARENT_VARIABLE)
    if main:
        os.environ[PARENT_VARIABLE] = identity
    began = time.time()
    clock = time.perf_counter()
    try:
        yield
    finally:
        seconds = time.perf_counter() - clock
        stack.pop()
        if main:
            _main.pop()
            if previous is None:
                os.environ.pop(PARENT_VARIABLE, None)
            else:
                os.environ[PARENT_VARIABLE] = previous
        line = json.dumps({"id": identity, "parent": parent, "name": name, "kind": kind,
                           "start": began, "seconds": seconds}) + "\n"
        with open(path, "ab", buffering=0) as output:
            output.write(line.encode())


def load(path):
    spans = [json.loads(line) for line in Path(path).read_text().splitlines() if line.strip()]
    for entry in spans:
        entry["children"] = []
    index = {entry["id"]: entry for entry in spans}
    roots = []
    for entry in sorted(spans, key=lambda item: item["start"]):
        parent = index.get(entry["parent"])
        (parent["children"] if parent else roots).append(entry)
    return spans, roots


def runs():
    return sorted(DIRECTORY.glob("*.jsonl")) if DIRECTORY.is_dir() else []


def render(path, *, depth=4, minimum=0.5, top=15):
    """Tree of the run (children slower than `minimum` seconds), then the slowest leaf spans."""
    spans, roots = load(path)
    if not spans:
        return f"{path.name}: no spans recorded"
    wall = max(entry["seconds"] for entry in roots)
    lines = [f"{path.name}: {wall:.1f}s wall, {len(spans)} spans"]

    def show(entry, level):
        hidden = [child for child in entry["children"] if child["seconds"] < minimum]
        inner = sum(child["seconds"] for child in entry["children"]) if entry["kind"] != "step" else 0
        lines.append(f"{'  ' * level}{entry['seconds']:7.1f}s {100 * entry['seconds'] / wall:4.0f}%  "
                     f"[{entry['kind']}] {entry['name']}")
        if level + 1 >= depth:
            return
        for child in sorted(entry["children"], key=lambda item: -item["seconds"]):
            if child["seconds"] >= minimum:
                show(child, level + 1)
        if hidden:
            lines.append(f"{'  ' * (level + 1)}{sum(c['seconds'] for c in hidden):7.1f}s        "
                         f"({len(hidden)} spans under {minimum}s)")

    for root in roots:
        show(root, 0)
    leaves = sorted((entry for entry in spans if not entry["children"]), key=lambda item: -item["seconds"])
    lines.append("")
    lines.append(f"Slowest {min(top, len(leaves))} leaf spans:")
    lines += [f"{entry['seconds']:7.1f}s  [{entry['kind']}] {entry['name']}" for entry in leaves[:top]]
    return "\n".join(lines)


def compare(newer, older, *, top=15):
    """Largest per-name changes between two runs of the same command."""
    def totals(path):
        sums = {}
        for entry in load(path)[0]:
            key = (entry["kind"], entry["name"])
            sums[key] = sums.get(key, 0) + entry["seconds"]
        return sums
    now, before = totals(newer), totals(older)
    deltas = sorted(((now.get(key, 0) - before.get(key, 0), key) for key in now.keys() | before.keys()),
                    key=lambda item: -abs(item[0]))
    lines = [f"{newer.name} vs {older.name}:"]
    lines += [f"{delta:+8.1f}s  [{kind}] {name}" for delta, (kind, name) in deltas[:top]]
    return "\n".join(lines)
