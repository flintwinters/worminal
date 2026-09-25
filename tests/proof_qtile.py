"""Check zoom alignment under Qtile on a private Xvfb display."""

from pathlib import Path
import subprocess
import sys
import time

from x11 import isolated_display


ROOT = Path(__file__).resolve().parent.parent


def main():
    with isolated_display() as env:
        log_path = ROOT / ".checks" / "qtile-proof.log"
        socket_path = ROOT / ".checks" / "qtile-proof.sock"
        with log_path.open("wb") as log:
            manager = subprocess.Popen(
                ["qtile", "start", "--backend", "x11", "--no-spawn",
                 "--config", str(ROOT / "tests/fixtures/qtile.py"),
                 "--socket", str(socket_path), "--log-path", str(log_path)],
                env=env, stdout=log, stderr=log,
            )
            try:
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline:
                    check = subprocess.run(
                        ["xprop", "-root", "_NET_SUPPORTING_WM_CHECK"],
                        env=env, capture_output=True, text=True,
                    )
                    if check.returncode == 0 and "window id #" in check.stdout:
                        break
                    if manager.poll() is not None:
                        raise RuntimeError("private Qtile exited during startup")
                    time.sleep(0.05)
                else:
                    raise RuntimeError("private Qtile did not become ready")
                code = subprocess.run(
                    [sys.executable, "-m", "unittest", "discover", "-s", "tests",
                     "-p", "test_native.py", "-k", "zoom_cursor_stays_in_cell", "-q"],
                    cwd=ROOT,
                    env={**env, "WORMINAL_PROOF_PRIVATE_DISPLAY": "1"},
                ).returncode
            except RuntimeError as error:
                print(f"{error}: {log_path.read_text(errors='replace')[-1500:]}",
                      file=sys.stderr)
                code = 1
            finally:
                if manager.poll() is None:
                    manager.terminate()
                try:
                    manager.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    manager.kill()
                    manager.wait()
        socket_path.unlink(missing_ok=True)
        log_path.unlink(missing_ok=True)
        return code


if __name__ == "__main__":
    sys.exit(main())
