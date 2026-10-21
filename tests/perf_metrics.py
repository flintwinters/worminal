"""Shared process-tree memory and CPU measurements for project benchmarks."""

import os
from pathlib import Path


def descendants(pid):
    result = {pid}
    try:
        children = Path(f"/proc/{pid}/task/{pid}/children").read_text().split()
    except FileNotFoundError:
        return result
    for child in children:
        result.update(descendants(int(child)))
    return result


def memory(pids):
    values = {"rss_kib": 0, "pss_kib": 0, "processes": 0}
    for pid in sorted(pids):
        try:
            lines = Path(f"/proc/{pid}/smaps_rollup").read_text().splitlines()
        except (FileNotFoundError, ProcessLookupError):
            continue
        values["processes"] += 1
        for line in lines:
            key, *parts = line.split()
            if key == "Rss:":
                values["rss_kib"] += int(parts[0])
            elif key == "Pss:":
                values["pss_kib"] += int(parts[0])
    return values


def cpu_time(pid):
    fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
    return (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")
