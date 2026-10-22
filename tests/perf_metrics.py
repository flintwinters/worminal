"""Shared process-tree memory and CPU measurements for project benchmarks."""

import os
from pathlib import Path


def descendants(pid):
    result = {pid}
    children = set()
    for thread in Path(f"/proc/{pid}/task").glob("*/children"):
        try:
            children.update(int(child) for child in thread.read_text().split())
        except (FileNotFoundError, ProcessLookupError):
            continue
    for child in children:
        result.update(descendants(child))
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


def main_thread_times(pid):
    """Main-thread runtime and runnable-queue wait in seconds (Linux schedstat)."""
    fields = Path(f"/proc/{pid}/schedstat").read_text().split()
    return int(fields[0]) / 1e9, int(fields[1]) / 1e9


def main_cpu_time(pid):
    """Main-thread scheduled runtime in seconds, from Linux's nanosecond counter."""
    return main_thread_times(pid)[0]


def startup_usage(pid):
    """Observed frontend/helper CPU and approximate process RSS high-water counters.

    Linux status VmHWM/VmRSS counters are approximate. Separate process peaks
    need not be simultaneous. See proc_pid_status(5); these are not peak PSS.
    Daemon-owned shells are outside this tree; this is not whole-app peak PSS.
    """
    processes = []
    for child in sorted(descendants(pid)):
        try:
            status = dict(line.split(":", 1) for line in Path(f"/proc/{child}/status").read_text().splitlines())
            if "VmHWM" not in status:
                continue  # An exited/reaped helper has no resident address space to sample.
            main_cpu, main_runqueue = main_thread_times(child)
            processes.append({"pid": child, "approx_rss_peak_kib": int(status["VmHWM"].split()[0]),
                              "rss_kib": int(status["VmRSS"].split()[0]),
                              "cpu_ms": cpu_time(child) * 1000,
                              "main_thread_cpu_ms": main_cpu * 1000,
                              "main_thread_runqueue_ms": main_runqueue * 1000})
        except (FileNotFoundError, ProcessLookupError):
            continue
    return {"processes": processes, "cpu_ms": sum(item["cpu_ms"] for item in processes),
            "main_thread_cpu_ms": sum(item["main_thread_cpu_ms"] for item in processes),
            "sum_approx_process_peak_rss_kib": sum(item["approx_rss_peak_kib"] for item in processes)}
