#!/usr/bin/env python3
"""Project checks: run `python3 manage.py check` after a coherent change."""

import subprocess
import sys


def main():
    if len(sys.argv) != 2 or sys.argv[1] not in {"check", "build", "test"}:
        print("Usage: python3 manage.py [check|build|test]")
        print("check runs the frontend build and Rust tests; build/test run one side.")
        return 2
    if sys.argv[1] in {"check", "build"}:
        if subprocess.run(["npm", "run", "build"]).returncode:
            return 1
    if sys.argv[1] in {"check", "test"}:
        if subprocess.run(["cargo", "test", "--manifest-path", "src-tauri/Cargo.toml"]).returncode:
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
