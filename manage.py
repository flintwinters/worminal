#!/usr/bin/env python3
"""Build, check, or run Worminal: run `python3 manage.py` for help."""

import os
import subprocess
import sys


def main():
    if len(sys.argv) != 2 or sys.argv[1] not in {"build", "check", "run", "clean"}:
        print("Usage: python3 manage.py [build|check|run|clean]")
        print("check builds and runs the native smoke test; run launches Worminal.")
        return 2

    command = sys.argv[1]
    if command == "clean":
        return subprocess.run(["make", "-s", "clean"]).returncode
    if subprocess.run(["make", "-s"]).returncode:
        return 1
    if command == "check":
        return subprocess.run(
            [sys.executable, "-m", "unittest", "discover", "-s", "tests", "-q"]
        ).returncode
    if command == "run":
        os.execv("./worminal", ["./worminal"])
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
