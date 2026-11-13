"""Exercise the public session-bus contract against real windows and PTYs."""
from runtime import CHECKS

from runtime import runtime_binary

from contextlib import contextmanager
import json
import os
from pathlib import Path
import shlex
import shutil
import socket
import subprocess
import tempfile
import time
import unittest
import uuid
from unittest.mock import patch

from x11 import close_window, isolated_display
from preview import profile
from workspace_protocol import stop_workspace

ROOT = Path(__file__).resolve().parent.parent
FRONTEND = runtime_binary(os.environ.get("WORMINAL_FRONTEND", "worminal"))
INTERFACE = "org.worminal.Windows1"
OBJECT = "/org/worminal/Windows"


@contextmanager
def private_bus(env):
    # Abstract sockets have no path: nested update storage under build/checks exceeds the 108-byte
    # sun_path limit, which left dbus-daemon silent and the address empty.
    address = f"unix:abstract=worminal-bus-{uuid.uuid4().hex}"
    daemon = subprocess.Popen(["dbus-daemon", "--session", "--nofork", "--print-address=1", f"--address={address}"],
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    try:
        printed = daemon.stdout.readline().strip()
        if not printed:
            raise RuntimeError(f"private session bus did not start: {daemon.stderr.read()}")
        yield {**env, "DBUS_SESSION_BUS_ADDRESS": printed}
    finally:
        daemon.terminate()
        daemon.communicate(timeout=5)


@unittest.skipUnless(all(shutil.which(tool) for tool in ("busctl", "dbus-daemon", "Xvfb", "xdotool")),
                     "session-bus and private-display tools are required")
class WindowStateTest(unittest.TestCase):
    def test_non_utf8_directory_is_published_as_original_bytes(self):
        with tempfile.TemporaryDirectory(prefix="window-state-bytes-", dir=CHECKS) as temporary, \
                isolated_display() as display, private_bus(display) as session, profile(session) as env:
            raw = os.fsencode(temporary) + b"/raw-\xff"
            os.mkdir(raw)
            env.update(GDK_BACKEND="x11", SHELL="/bin/sh")
            with (Path(temporary) / "frontend.log").open("wb") as log:
                app = subprocess.Popen([str(FRONTEND), "-e", "/bin/cat"], cwd=raw, env=env, stdout=log, stderr=log)
                try:
                    def exact_directory():
                        result = subprocess.run(["busctl", "--user", "--json=short", "call",
                            f"org.worminal.Instance.p{app.pid}", OBJECT, INTERFACE, "GetWindows"],
                            env=env, capture_output=True, text=True, timeout=3)
                        if result.returncode != 0: return None
                        rows = json.loads(result.stdout)["data"][0]
                        return rows if len(rows) == 1 and rows[0][2] and bytes(rows[0][3]) == raw else None
                    self.wait_for(exact_directory, "unmodified non-UTF-8 directory bytes")
                finally:
                    app.terminate(); app.wait(timeout=5)
                    stop_workspace(env)

    def wait_for(self, callback, description):
        deadline = time.monotonic() + 8
        last = None
        while time.monotonic() < deadline:
            last = callback()
            if last:
                return last
            time.sleep(.03)
        self.fail(f"Timed out waiting for {description}; last result: {last!r}")

    def test_directory_selection_focus_unavailability_and_signals(self):
        checks = CHECKS
        checks.mkdir(parents=True, exist_ok=True)
        # Stop private services before removing directories their shells may still use.
        with tempfile.TemporaryDirectory(prefix="window-state-", dir=checks) as temporary, \
                isolated_display() as display, private_bus(display) as session, profile(session) as env:
            base = Path(temporary)
            first, second = base / "first λ space", base / "second space"
            first.mkdir()
            second.mkdir()
            env.update(GDK_BACKEND="x11", SHELL="/bin/sh")
            # The loop changes cwd with absolutely no PTY output after setup.
            ready = base / "ready"
            command = (f'stty -echo; touch {shlex.quote(str(ready))}; '
                       'while IFS= read -r directory; do cd "$directory"; done')
            with (base / "frontend.log").open("w+") as log, (base / "signals.log").open("w+") as signals:
                app = subprocess.Popen([str(FRONTEND), "-T", "Window state fixture",
                                        "-e", "/bin/sh", "-c", command], cwd=first, env=env,
                                       stdout=log, stderr=log)
                name = f"org.worminal.Instance.p{app.pid}"
                monitor = subprocess.Popen(["gdbus", "monitor", "--session", "--dest", name,
                                            "--object-path", OBJECT], env=env, stdout=signals, stderr=signals)
                try:
                    def snapshot():
                        result = subprocess.run(["busctl", "--user", "--json=short", "call", name,
                                                 OBJECT, INTERFACE, "GetWindows"], env=env,
                                                capture_output=True, text=True, timeout=3)
                        return json.loads(result.stdout)["data"][0] if result.returncode == 0 else []

                    def directory(path, available=True):
                        rows = snapshot()
                        return rows if len(rows) == 1 and rows[0][2] == available and \
                            bytes(rows[0][3]) == os.fsencode(path) else None

                    self.wait_for(lambda: directory(first), "initial full directory")
                    window = subprocess.check_output(["xdotool", "search", "--onlyvisible", "--name",
                                                      "^Window state fixture$"], env=env, text=True).splitlines()[0]

                    def key(*args):
                        subprocess.run(["xdotool", *args], env=env, check=True, timeout=3)

                    key("windowfocus", "--sync", window)
                    self.wait_for(lambda: snapshot()[0][1], "focused window")
                    self.wait_for(ready.exists, "silent shell readiness")

                    def signal_count():
                        return (base / "signals.log").read_text().count(INTERFACE + ".Changed")

                    self.wait_for(signal_count, "initial state notification")
                    before = signal_count()
                    key("type", "--clearmodifiers", "--delay", "0", str(second))
                    key("key", "Return")
                    self.wait_for(lambda: directory(second), "silent directory change")
                    self.wait_for(lambda: signal_count() > before, "silent directory notification")
                    key("key", "ctrl+t")
                    self.wait_for(lambda: directory(second), "new tab directory")
                    key("type", "--clearmodifiers", "--delay", "0", f"cd {shlex.quote(str(base))}")
                    key("key", "Return")
                    self.wait_for(lambda: directory(base), "second tab directory")
                    key("key", "ctrl+1")
                    self.wait_for(lambda: directory(second), "switch to first tab")
                    key("key", "ctrl+2")
                    self.wait_for(lambda: directory(base), "switch to second tab")
                    key("type", "--clearmodifiers", "--delay", "0",
                        f"{FRONTEND} --new-browser-tab about:blank")
                    key("key", "Return")
                    self.wait_for(lambda: directory("", False), "browser unavailability")
                    key("key", "ctrl+1")
                    self.wait_for(lambda: directory(second), "terminal availability restored")
                    original_id = snapshot()[0][0]
                    subprocess.run([str(FRONTEND), "-e", "/bin/cat"],
                                   env=env, check=True, timeout=5)
                    self.wait_for(lambda: len(snapshot()) == 2, "second window registration")
                    windows = subprocess.check_output(["xdotool", "search", "--onlyvisible", "--pid",
                                                       str(app.pid)], env=env, text=True).splitlines()
                    second_window = next(identity for identity in windows if identity != window)
                    key("windowfocus", "--sync", second_window)

                    def focused_window(original):
                        rows = snapshot()
                        return len(rows) == 2 and sum(row[1] for row in rows) == 1 and any(
                            row[1] and (row[0] == original_id) == original for row in rows)

                    self.wait_for(lambda: focused_window(False), "second window focus")
                    second_title = subprocess.check_output(["xdotool", "getwindowname", second_window],
                                                           env=env, text=True).strip()
                    key("windowfocus", "--sync", window)
                    self.wait_for(lambda: focused_window(True), "focus returning to first window")
                    close_window(env, second_title)
                    self.wait_for(lambda: directory(second), "closed window removed from snapshot")
                    subprocess.run([str(runtime_binary("worminald")), "--stop"], env=env, check=True, timeout=3)
                    self.wait_for(lambda: directory("", False), "disconnected unavailability")
                    self.assertIn(INTERFACE + ".Changed", (base / "signals.log").read_text())
                    self.assertIsNone(app.poll(), (base / "frontend.log").read_text())
                finally:
                    monitor.terminate()
                    monitor.wait(timeout=5)
                    if app.poll() is None:
                        app.terminate()
                    app.wait(timeout=5)
                    # A disconnected window can restart its daemon after the intentional --stop.
                    # Reap that scoped owner while profile/cache paths still exist: outer display
                    # cleanup runs after profile deletion, too late for mapped NFS .nfs files.
                    stop_workspace(env)

    def test_reconnected_owner_exits_before_profile_cleanup(self):
        original_profile = profile

        @contextmanager
        def checked_profile(session):
            with original_profile(session) as env:
                try:
                    yield env
                finally:
                    try:
                        self.assertTrue(Path(env["XDG_RUNTIME_DIR"]).is_dir())
                        with socket.socket(socket.AF_UNIX) as peer:
                            endpoint = f"\0worminal-workspace-v2-{os.getuid()}-{env['WORMINAL_SHARED_SOCKET_SCOPE']}"
                            with self.assertRaises(ConnectionRefusedError,
                                                   msg="fixture owner survived until profile deletion"):
                                peer.connect(endpoint)
                    finally:
                        # Keep a deliberately broken cleanup mutation from racing rmtree itself.
                        stop_workspace(env)

        real_popen = subprocess.Popen
        owners = []

        def spawn(*args, **kwargs):
            process = real_popen(*args, **kwargs)
            if args[0][:2] == [str(FRONTEND), "-T"]:
                original_wait = process.wait

                def wait(*wait_args, **wait_kwargs):
                    result = original_wait(*wait_args, **wait_kwargs)
                    # Force the reconnect owner to exist at GUI exit rather than depending on
                    # its timer winning the race with fixture teardown.
                    owner = real_popen([str(runtime_binary("worminald")), "--serve"], env=kwargs["env"],
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                    owners.append(owner)
                    endpoint = f"\0worminal-workspace-v2-{os.getuid()}-{kwargs['env']['WORMINAL_SHARED_SOCKET_SCOPE']}"
                    def listening():
                        with socket.socket(socket.AF_UNIX) as peer:
                            try: peer.connect(endpoint)
                            except ConnectionRefusedError: return False
                            return True
                    self.wait_for(listening, "reconnected fixture daemon")
                    return result
                process.wait = wait
            return process

        try:
            with patch(__name__ + ".profile", checked_profile), patch.object(subprocess, "Popen", spawn):
                self.test_directory_selection_focus_unavailability_and_signals()
        finally:
            for owner in owners:
                owner.wait(timeout=5)
