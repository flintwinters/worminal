"""Trace a private startup through the shared visible-readiness fixture, never the desktop."""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import time

from Xlib import X, display
from rich.console import Console

from visible_latency import launch, stop
from x11 import close_window, isolated_display

ROOT = Path(__file__).resolve().parent.parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--shell", action="store_true")
    args = parser.parse_args()
    if not shutil.which("strace"):
        parser.error("Install strace to profile startup")
    checks = ROOT / ".checks"
    checks.mkdir(exist_ok=True)
    trace = checks / "startup-syscalls.log"
    log_path = checks / "startup-profile.log"
    process = None
    with isolated_display() as env, log_path.open("w+b") as log:
        env.update(GDK_BACKEND="x11", WORMINAL_TRACE_TIMING="1")
        if args.shell:
            env["WORMINAL_LATENCY_SHELL"] = os.environ.get("SHELL", "/bin/sh")
        connection = display.Display(env["DISPLAY"])
        try:
            title = f"Startup profile {os.getpid()}"
            started = time.time_ns()
            process, window, timing = launch(env, connection, title, log,
                prefix=["strace", "--kill-on-exit", "-f", "-ttt", "-T", "-yy", "-s", "256", "-o", str(trace)])
            ready = time.time_ns()
            identity = window.get_full_property(connection.intern_atom("_NET_WM_PID"), X.AnyPropertyType)
            if identity is None or len(identity.value) != 1:
                raise RuntimeError("Ready window lacks a unique owner PID")
            report = {"revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT,
                                                            text=True).strip(),
                      "owner_pid": int(identity.value[0]), "started_unix_ns": started, "ready_unix_ns": ready,
                      "traced_visible_ms": timing, "interactive_shell": env.get("WORMINAL_LATENCY_SHELL"),
                      "gtk_environment": {key: env[key] for key in
                                          ("GTK_MODULES", "GTK3_MODULES", "GTK_THEME", "GTK_IM_MODULE") if key in env},
                      "trace": str(trace.relative_to(ROOT)), "milestones": str(log_path.relative_to(ROOT)),
                      "notes": "Diagnostic strace timings include substantial tracing overhead. "
                               "Trace includes private owner threads, daemon and shell descendants through cleanup. "
                               "Use ready_unix_ns as the startup cutoff; use latency-visible for performance."}
            close_window(env, title)
        finally:
            connection.close()
            if process:
                # --kill-on-exit also contains failures to this private trace tree.
                stop(process)
    (checks / "startup-profile.json").write_text(json.dumps(report, indent=2) + "\n")
    Console().print("Diagnostic startup trace: .checks/startup-syscalls.log")
    Console().print("Owner PID, readiness cutoff and trace limitations: .checks/startup-profile.json")


if __name__ == "__main__":
    main()
