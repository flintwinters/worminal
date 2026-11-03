"""Exercise the shipped launcher/module boundary through an isolated SSH bridge."""
from runtime import CHECKS

from runtime import runtime_binary


import os
from pathlib import Path
import select
import signal
import subprocess
import time
import unittest

from x11 import isolated_display


ROOT = Path(__file__).resolve().parent.parent


class MasterStartupTest(unittest.TestCase):
    def test_master_launcher_accepts_keyboard_input(self):
        executable = Path(os.environ.get("WORMINAL_TEST_EXECUTABLE", runtime_binary("worminal")))
        with isolated_display() as env:
            if master := env.get("WORMINAL_TEST_REAL_MASTER"):
                self.addCleanup(subprocess.run,
                    ["/usr/bin/ssh", "-T", "-oBatchMode=yes", "-oConnectTimeout=5", master,
                     f"env WORMINAL_SHARED_SOCKET_SCOPE={env['WORMINAL_SHARED_SOCKET_SCOPE']} worminald --stop"],
                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=15)
            env.update(GDK_BACKEND="x11", NO_AT_BRIDGE="1", GTK_USE_PORTAL="0",
                       WORMINAL_TEST_DAEMON=env.get("WORMINAL_TEST_DAEMON", str(runtime_binary("worminald"))),
                       PATH=str(ROOT / "tests/fixtures/preview-master") + os.pathsep + env["PATH"])
            env.pop("DBUS_SESSION_BUS_ADDRESS", None)
            log_path = CHECKS / "master-startup.log"
            with log_path.open("wb") as log:
                process = subprocess.Popen(
                    [str(executable), "--master", "preview-test", "-T", "Master startup check",
                     "-o", "-", "-e", "/bin/sh", "-c",
                     "stty raw -echo; printf MASTER_READY; "
                     "dd bs=1 count=1 2>/dev/null; printf MASTER_DONE"],
                    env=env, stdout=subprocess.PIPE, stderr=log, start_new_session=True)
                output = b""

                def expect(marker):
                    nonlocal output
                    deadline = time.monotonic() + 10
                    while marker not in output and time.monotonic() < deadline:
                        ready, _, _ = select.select([process.stdout], [], [],
                                                    max(0, deadline - time.monotonic()))
                        if ready:
                            chunk = os.read(process.stdout.fileno(), 4096)
                            if not chunk:
                                break
                            output += chunk
                    self.assertIn(marker, output, f"See {log_path}; status={process.poll()}")

                try:
                    expect(b"MASTER_READY")
                    window = subprocess.check_output(
                        ["xdotool", "search", "--sync", "--onlyvisible", "--pid", str(process.pid)],
                        env=env, text=True, timeout=10).splitlines()[0]
                    subprocess.run(["xdotool", "windowfocus", "--sync", window,
                                    "key", "--clearmodifiers", "x"],
                                   env=env, check=True, timeout=5, stdout=log, stderr=log)
                    expect(b"xMASTER_DONE")
                    self.assertEqual(process.wait(timeout=10), 0)
                finally:
                    # Reap every helper even if startup hangs before a window exists.
                    try:
                        os.killpg(process.pid, signal.SIGTERM)
                    except ProcessLookupError:
                        pass
                    try:
                        process.communicate(timeout=5)
                    except subprocess.TimeoutExpired:
                        os.killpg(process.pid, signal.SIGKILL)
                        process.communicate(timeout=5)
