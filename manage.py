#!/usr/bin/env python3
"""Project checks and Tauri launcher: run `python3 manage.py` for help."""

import os
from pathlib import Path
import subprocess
import sys


def rust_env():
    env = os.environ.copy()
    rustup_bin = Path.home() / ".cargo" / "bin"
    rustup = rustup_bin / ("rustup.exe" if os.name == "nt" else "rustup")
    if rustup.is_file():
        env["PATH"] = os.pathsep.join([str(rustup_bin), env.get("PATH", "")])
    return env


def main():
    if len(sys.argv) < 2 or sys.argv[1] not in {"check", "build", "test", "tauri"}:
        print("Usage: python3 manage.py [check|build|test|tauri <args>]")
        print("check builds the frontend and runs Rust tests; tauri runs the desktop CLI.")
        return 2
    command = sys.argv[1]
    if command == "tauri":
        if len(sys.argv) < 3:
            print("Usage: python3 manage.py tauri [dev|build|info]")
            return 2
        launcher = Path("node_modules/.bin") / ("tauri.cmd" if os.name == "nt" else "tauri")
        return subprocess.run([str(launcher), *sys.argv[2:]], env=rust_env()).returncode
    if len(sys.argv) != 2:
        return 2
    if command in {"check", "build"}:
        if subprocess.run(["npm", "run", "build"]).returncode:
            return 1
    if command in {"check", "test"}:
        if subprocess.run([sys.executable, "-m", "unittest", "discover", "-s", "tests", "-q"]).returncode:
            return 1
        if subprocess.run(["cargo", "test", "--manifest-path", "src-tauri/Cargo.toml"], env=rust_env()).returncode:
            return 1
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
