#!/usr/bin/env python3
"""Build, check, benchmark, or run Worminal: run `python3 manage.py` for help."""

import os
import subprocess
import sys


def main():
    if len(sys.argv) != 2 or sys.argv[1] not in {"build", "check", "latency", "latency-shell", "run", "clean"}:
        print("Usage: python3 manage.py [build|check|latency|latency-shell|run|clean]")
        print("latency measures /bin/cat; latency-shell measures your interactive shell.")
        print("Both run on private Xvfb, away from your desktop.")
        return 2

    command = sys.argv[1]
    if command == "clean":
        return subprocess.run(["make", "-s", "clean"]).returncode
    if subprocess.run(["make", "-s"]).returncode:
        return 1
    if command in {"check", "latency", "latency-shell"}:
        if subprocess.run(["make", "-s", ".checks/key_injector"]).returncode:
            return 1
    if command == "check":
        return subprocess.run(
            [sys.executable, "-m", "unittest", "discover", "-s", "tests", "-q"]
        ).returncode
    if command in {"latency", "latency-shell"}:
        args = [sys.executable, "tests/latency.py"]
        if command == "latency-shell":
            args.append("--shell")
        return subprocess.run(args).returncode
    if command == "run":
        os.execv("./worminal", ["./worminal"])
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
