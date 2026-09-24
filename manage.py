#!/usr/bin/env python3
"""Build, check, benchmark, or run Worminal: run `python3 manage.py` for help."""

import os
import subprocess
import sys

from tools.theme import generate_theme
from tools.icon import generate_icon


def main():
    if len(sys.argv) != 2 or sys.argv[1] not in {"build", "check", "latency", "latency-shell", "run", "clean", "icon", "proof-desktop"}:
        print("Usage: python3 manage.py [build|check|latency|latency-shell|run|clean|icon|proof-desktop]")
        print("latency measures /bin/cat; latency-shell measures your interactive shell.")
        print("Both run on private Xvfb, away from your desktop.")
        print("icon regenerates the embedded X11 icon from worminal.svg (needs Inkscape and Pillow).")
        print("proof-desktop briefly opens two test views on your current X11 desktop.")
        return 2

    command = sys.argv[1]
    if command == "icon":
        try:
            generate_icon()
        except (OSError, ImportError, subprocess.CalledProcessError) as error:
            print(f"Icon error: {error}", file=sys.stderr)
            return 1
        return 0
    if command == "clean":
        return subprocess.run(["make", "-s", "clean"]).returncode
    if command == "run":
        if not os.path.isfile("./worminal"):
            print("Build Worminal first with: python3 manage.py build")
            return 1
        os.execv("./worminal", ["./worminal"])
    try:
        generate_theme()
    except (OSError, ValueError) as error:
        print(f"Theme error: {error}", file=sys.stderr)
        return 1
    if subprocess.run(["make", "-s"]).returncode:
        return 1
    if command == "proof-desktop":
        if subprocess.run(["make", "-s", ".checks/view_state_test"]).returncode:
            return 1
        return subprocess.run(
            [".checks/view_state_test"],
            env={**os.environ, "WORMINAL_PROOF_REQUIRE_IM": "1"},
        ).returncode
    if command in {"check", "latency", "latency-shell"}:
        if subprocess.run(["make", "-s", ".checks/key_injector"]).returncode:
            return 1
    if command == "check":
        if subprocess.run(["make", "-s", ".checks/placement_wm"]).returncode:
            return 1
        if subprocess.run(["make", "-s", ".checks/compact-worminal", ".checks/overlap_probe"]).returncode:
            return 1
        if subprocess.run(["make", "-s", ".checks/border_probe"]).returncode:
            return 1
        if subprocess.run(["make", "-s", ".checks/icon_probe"]).returncode:
            return 1
        if subprocess.run(["make", "-s", ".checks/view_state_test"]).returncode:
            return 1
        if subprocess.run(["make", "-s", ".checks/scrollback_test"]).returncode:
            return 1
        if subprocess.run([".checks/scrollback_test"],
                          env={**os.environ, "ASAN_OPTIONS": "detect_leaks=0"}).returncode:
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
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
