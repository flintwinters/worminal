"""Verify actual staged artifacts and native module resolution without publishing them."""

import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import time

from preview import profile
from workspace_protocol import service_pid
from x11 import close_window, isolated_display

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
from tools import build


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    stage = ROOT / ".checks/install-stage"
    previous = os.environ.get("DESTDIR")
    os.environ["DESTDIR"] = str(stage)
    try:
        build.install(Path("/usr/local"), skip_build=True)
        files = build.installation_files("/usr/local")
    finally:
        if previous is None:
            os.environ.pop("DESTDIR", None)
        else:
            os.environ["DESTDIR"] = previous
    records = []
    for source, target, mode in files:
        expected = (source.read_text().replace("VERSION", build.VERSION).encode()
                    if source.suffix == ".1" else source.read_bytes())
        if target.read_bytes() != expected or target.stat().st_mode & 0o777 != mode:
            raise AssertionError(f"Staged artifact differs from verified build: {target}")
        records.append({"path": str(target.relative_to(stage)), "sha256": digest(target)})
    prefix = stage / "usr/local"
    executable = prefix / "bin/worminal"
    for name, option, status in (("worminal", "--version", 0), ("worminal-x11", "-v", 1)):
        result = subprocess.run([str(prefix / "bin" / name), option],
                                capture_output=True, text=True, timeout=5)
        if result.returncode != status or build.VERSION not in result.stdout + result.stderr:
            raise AssertionError(f"Staged {name} version failed: {result.stderr}")
    for name in ("worminald", "worminald-x11"):
        result = subprocess.run([str(prefix / "bin" / name), "--invalid"],
                                capture_output=True, text=True, timeout=5)
        if result.returncode != 2 or "--serve|--bridge|--stop" not in result.stderr:
            raise AssertionError(f"Staged {name} CLI validation failed: {result.stderr}")
    env = {**os.environ, "WORMINAL_TEST_EXECUTABLE": str(executable),
           "WORMINAL_TEST_DAEMON": str(prefix / "bin/worminald")}
    subprocess.run([sys.executable, "-m", "unittest", "discover", "-s", "tests",
                    "-p", "test_master_startup.py", "-q"], env=env, check=True, timeout=60)
    title = "Staged browser verification"
    with isolated_display() as display, profile(display) as env:
        env.update(GDK_BACKEND="x11", GTK_USE_PORTAL="0")
        env.pop("WAYLAND_DISPLAY", None)
        env.pop("DBUS_SESSION_BUS_ADDRESS", None)
        with (ROOT / ".checks/install-browser.log").open("wb") as log:
            owner = subprocess.Popen([str(executable), "-T", title, "--surf", "about:blank"],
                                     env=env, stdout=log, stderr=log, start_new_session=True)
            try:
                frontend = prefix / "lib/worminal/frontend.so"
                browser_digest = digest(prefix / "lib/worminal" / build.BROWSER_MODULE)
                deadline = time.monotonic() + 15
                while time.monotonic() < deadline:
                    if owner.poll() is not None:
                        raise AssertionError("Staged browser exited; see .checks/install-browser.log")
                    maps = Path(f"/proc/{owner.pid}/maps").read_text()
                    # Browser generations load sealed snapshots, retaining their code across replacement.
                    # Verify both the mapped digest and the actual bytes behind that resident descriptor.
                    browser_loaded = False
                    if f"/memfd:{browser_digest}" in maps:
                        for descriptor in Path(f"/proc/{owner.pid}/fd").iterdir():
                            try:
                                if (f"/memfd:{browser_digest}" in os.readlink(descriptor)
                                        and digest(descriptor) == browser_digest):
                                    browser_loaded = True
                                    break
                            except FileNotFoundError:
                                continue
                    visible = subprocess.run(["xdotool", "search", "--onlyvisible", "--pid", str(owner.pid)],
                                             env=env, capture_output=True, timeout=2)
                    if str(frontend) in maps and browser_loaded and visible.returncode == 0:
                        break
                    time.sleep(.02)
                else:
                    (ROOT / ".checks/install-browser-maps.log").write_text(maps)
                    raise AssertionError("Staged browser did not map verified modules and a visible window")
                with socket.socket(socket.AF_UNIX) as connection:
                    connection.settimeout(3)
                    connection.connect(f"\0worminal-workspace-v2-{os.getuid()}-"
                                       f"{env['WORMINAL_SHARED_SOCKET_SCOPE']}")
                    daemon_pid = service_pid(connection)
                daemon = prefix / "bin/worminald"
                if Path(f"/proc/{daemon_pid}/exe").resolve() != daemon:
                    raise AssertionError("Staged launcher started a daemon from another installation")
                close_window(env, title)
                if owner.wait(timeout=10) != 0:
                    raise AssertionError("Staged browser did not close gracefully")
            finally:
                # This group was created above, on this private display and private profile only.
                try:
                    os.killpg(owner.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    owner.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(owner.pid, signal.SIGKILL)
                    owner.wait(timeout=5)
                subprocess.run([str(prefix / "bin/worminald"), "--stop"], env=env,
                               stdout=log, stderr=log, timeout=5)
    (ROOT / ".checks/install-verification.json").write_text(json.dumps(records, indent=2) + "\n")
    print("Staged /usr/local: hashes, modes, native CLI, SSH input, daemon and lazy module paths passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
