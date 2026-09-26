"""Run shared-tab checks against remote Cinnamon on a private local Xvfb display."""

from pathlib import Path
import select
import subprocess
import sys
import time

from x11 import isolated_display


ROOT = Path(__file__).resolve().parent.parent
HOST = "felix@192.168.32.193"
SSH = ["ssh", "-F", "/dev/null", "-Y", "-o", "BatchMode=yes",
       "-o", "ConnectTimeout=8", "-o", "ExitOnForwardFailure=yes", HOST]


def wait_for_property(env, name, seconds, manager, contains=None):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        result = subprocess.run(["xprop", "-root", name], env=env,
                                capture_output=True, text=True)
        if (result.returncode == 0 and "not found" not in result.stdout
                and (contains is None or contains in result.stdout)):
            return
        if manager.poll() is not None:
            raise RuntimeError("Cinnamon exited during startup")
        time.sleep(.05)
    raise RuntimeError(f"Cinnamon proof timed out waiting for {name}")


def main():
    with isolated_display() as env:
        # SSH forwards this private DISPLAY. Neither Cinnamon nor test windows
        # can reach the remote host's active :0 session.
        preflight = subprocess.run([*SSH, "xprop -root >/dev/null"], env=env,
                                   capture_output=True, text=True)
        if preflight.returncode:
            raise RuntimeError(f"private X11 forwarding failed: {preflight.stderr.strip()}")

        env = {**env, "XMODIFIERS": "@im=ibus"}
        log_path = ROOT / "build" / "cinnamon-proof.log"
        with log_path.open("wb") as log:
            ibus = subprocess.Popen(
                ["dbus-run-session", "--", "ibus-daemon", "--single", "--xim",
                 "--panel=disable", "--emoji-extension=disable", "--config=disable"],
                env=env, stdout=log, stderr=log,
            )
            manager = None
            remote_pid = None
            try:
                # The reported PID belongs to timeout, which forwards SIGTERM
                # to Cinnamon. A deadline also bounds cleanup if SSH drops.
                manager = subprocess.Popen(
                    [*SSH, "sh -c 'timeout 75s env LIBGL_ALWAYS_SOFTWARE=1 "
                     "XDG_SESSION_TYPE=x11 dbus-run-session -- cinnamon --replace & "
                     "wm=$!; echo $wm; wait $wm'"],
                    env=env, stdout=subprocess.PIPE, stderr=log,
                )
                ready, _, _ = select.select([manager.stdout], [], [], 10)
                remote_pid = manager.stdout.readline().strip() if ready else b""
                if not remote_pid.isdigit():
                    raise RuntimeError("remote Cinnamon did not report its PID")

                wait_for_property(env, "_NET_SUPPORTING_WM_CHECK", 20, manager)
                wait_for_property(env, "_NET_SUPPORTED", 20, manager,
                                  contains="_NET_ACTIVE_WINDOW")
                wait_for_property(env, "_XIM_SERVERS", 10, manager)
                code = subprocess.run(
                    [str(ROOT / "build/view_state_test")],
                    env={**env, "WORMINAL_PROOF_REQUIRE_IM": "1",
                         "WORMINAL_PROOF_MANAGED_VIEWS": "1"},
                ).returncode
                if not code:
                    code = subprocess.run(
                        [sys.executable, "-m", "unittest", "discover", "-s", "tests",
                         "-p", "test_native.py", "-k", "shared_view_end_to_end",
                         "-k", "independent_tabs_share_one_owner", "-q"],
                        cwd=ROOT, env={**env, "WORMINAL_PROOF_PRIVATE_DISPLAY": "1"},
                    ).returncode
            except RuntimeError as error:
                print(f"{error}: {log_path.read_text(errors='replace')[-2000:]}",
                      file=sys.stderr)
                code = 1
            finally:
                if remote_pid and remote_pid.isdigit():
                    subprocess.run([*SSH, f"kill -TERM {remote_pid.decode()}"],
                                   env=env, capture_output=True, timeout=10)
                if manager is not None:
                    try:
                        manager.communicate(timeout=5)
                    except subprocess.TimeoutExpired:
                        manager.kill()
                        manager.communicate()
                ibus.terminate()
                try:
                    ibus.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    ibus.kill()
                    ibus.wait()
        log_path.unlink(missing_ok=True)
        return code


if __name__ == "__main__":
    sys.exit(main())
