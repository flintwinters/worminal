"""Run shared-tab checks under KWin on a private Xvfb display."""

from pathlib import Path
import subprocess
import sys
import time

from x11 import isolated_display


ROOT = Path(__file__).resolve().parent.parent


def main():
    with isolated_display() as env:
        env = {**env, "XMODIFIERS": "@im=ibus"}
        log_path = ROOT / "build" / "kwin-proof.log"
        with log_path.open("wb") as log:
            manager = subprocess.Popen(
                ["dbus-run-session", "--", "sh", "-c",
                 "ibus-daemon --daemonize --single --xim --panel=disable "
                 "--emoji-extension=disable --config=disable; exec kwin_x11 --replace"],
                env=env, stdout=log, stderr=log,
            )
            try:
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline:
                    check = subprocess.run(["xprop", "-root", "_NET_SUPPORTING_WM_CHECK"],
                                           env=env, capture_output=True, text=True)
                    if check.returncode == 0 and "window id #" in check.stdout:
                        break
                    if manager.poll() is not None:
                        raise RuntimeError("private KWin exited during startup")
                    time.sleep(.05)
                else:
                    raise RuntimeError("private KWin did not become ready")

                deadline = time.monotonic() + 10
                while time.monotonic() < deadline:
                    supported = subprocess.run(["xprop", "-root", "_NET_SUPPORTED"],
                                               env=env, capture_output=True, text=True)
                    if "_NET_ACTIVE_WINDOW" in supported.stdout:
                        break
                    if manager.poll() is not None:
                        raise RuntimeError("private KWin exited during startup")
                    time.sleep(.05)
                else:
                    raise RuntimeError("private KWin did not advertise window activation")

                deadline = time.monotonic() + 5
                while time.monotonic() < deadline:
                    xim = subprocess.run(["xprop", "-root", "_XIM_SERVERS"],
                                         env=env, capture_output=True, text=True)
                    if xim.returncode == 0 and "not found" not in xim.stdout:
                        break
                    time.sleep(.05)
                else:
                    raise RuntimeError("private IBus XIM did not become ready")

                result = subprocess.run(
                    [str(ROOT / "build/view_state_test")],
                    env={**env, "WORMINAL_PROOF_REQUIRE_IM": "1",
                         "WORMINAL_PROOF_MANAGED_VIEWS": "1"},
                )
                code = result.returncode
                if not code:
                    code = subprocess.run(
                        [sys.executable, "-m", "unittest", "discover", "-s", "tests",
                         "-p", "test_native.py", "-k", "shared_view_end_to_end",
                         "-k", "independent_tabs_share_one_owner", "-q"],
                        cwd=ROOT,
                        env={**env, "WORMINAL_PROOF_PRIVATE_DISPLAY": "1"},
                    ).returncode
            except RuntimeError as error:
                print(f"{error}: {log_path.read_text(errors='replace')[-2000:]}", file=sys.stderr)
                code = 1
            finally:
                if manager.poll() is None:
                    manager.terminate()
                try:
                    manager.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    manager.kill()
                    manager.wait()
        log_path.unlink(missing_ok=True)
        return code


if __name__ == "__main__":
    sys.exit(main())
