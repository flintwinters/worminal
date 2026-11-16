"""OSC 99 notifications from a real PTY reach a private notification service; its click selects the tab."""
from runtime import CHECKS, runtime_binary

import importlib.util
import json
import os
from pathlib import Path
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest

from preview import profile
from test_window_state import INTERFACE, OBJECT, private_bus
from workspace_protocol import stop_workspace
from x11 import isolated_display

FRONTEND = runtime_binary(os.environ.get("WORMINAL_FRONTEND", "worminal"))
SERVICE = Path(__file__).with_name("notification_service.py")
# Each step waits for its trigger file, so the test controls which tab and focus state it happens in.
SCRIPT = r"""
step() { while [ ! -e "$WORMINAL_NOTIFICATION_FIXTURE_DIR/$1" ]; do sleep 0.05; done; shift; printf "$@"; }
step focused '\033]99;i=quiet:o=unfocused;suppressed while focused\033\\'
step focused '\033]99;i=a:d=0;Fixture <title>\033\\\033]99;i=a:p=body:u=2;needs <b>you</b>\tnow\033\\'
step hidden '\033]99;i=b:o=invisible:e=1;%s\033\\' "$(printf 'second notice' | base64)"
exec cat
"""


@unittest.skipUnless(all(shutil.which(tool) for tool in ("busctl", "dbus-daemon", "Xvfb", "xdotool"))
                     and importlib.util.find_spec("gi"),
                     "session-bus, PyGObject and private-display tools are required")
class NotificationTest(unittest.TestCase):
    def wait_for(self, callback, description):
        deadline = time.monotonic() + 8
        last = None
        while time.monotonic() < deadline:
            last = callback()
            if last:
                return last
            time.sleep(.03)
        self.fail(f"Timed out waiting for {description}; last result: {last!r}")

    def test_text_occasion_replacement_and_click(self):
        CHECKS.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="notifications-", dir=CHECKS) as temporary, \
                isolated_display() as display, private_bus(display) as session, profile(session) as env:
            base = Path(temporary)
            first, second, calls = base / "first", base / "second", base / "calls.jsonl"
            first.mkdir()
            second.mkdir()
            # Keep the first tab's cwd stable; the default shell marks a new tab ready by
            # entering its own directory before the public window state reports it selected.
            shell = base / "second-shell"
            shell.write_text("#!/bin/sh\ncd " + shlex.quote(str(second)) + "\nexec /bin/sh\n")
            shell.chmod(0o700)
            env.update(GDK_BACKEND="x11", SHELL=str(shell),
                       WORMINAL_NOTIFICATION_FIXTURE_DIR=str(base))
            with (base / "frontend.log").open("w+") as log:
                service = subprocess.Popen([sys.executable, str(SERVICE), str(calls)], env=env,
                                           stdout=log, stderr=log)
                self.wait_for(lambda: subprocess.run(
                    ["busctl", "--user", "status", "org.freedesktop.Notifications"], env=env,
                    capture_output=True).returncode == 0, "private notification service")
                app = subprocess.Popen([str(FRONTEND), "-T", "Notification fixture", "-e", "/bin/sh", "-c",
                                        SCRIPT], cwd=first, env=env,
                                       stdout=log, stderr=log)
                name = f"org.worminal.Instance.p{app.pid}"
                try:
                    def directory(path):
                        result = subprocess.run(["busctl", "--user", "--json=short", "call", name,
                                                 OBJECT, INTERFACE, "GetWindows"], env=env,
                                                capture_output=True, text=True, timeout=3)
                        rows = json.loads(result.stdout)["data"][0] if result.returncode == 0 else []
                        return len(rows) == 1 and rows[0][1] and bytes(rows[0][3]) == os.fsencode(path)

                    def notices(count):
                        lines = calls.read_text().splitlines() if calls.exists() else []
                        return [json.loads(line) for line in lines] if len(lines) >= count else None

                    def key(*args):
                        subprocess.run(["xdotool", *args], env=env, check=True, timeout=3)

                    self.wait_for(lambda: subprocess.run(
                        ["xdotool", "search", "--onlyvisible", "--name", "^Notification fixture$"],
                        env=env, capture_output=True, text=True).stdout.split(), "fixture window")
                    window = subprocess.check_output(["xdotool", "search", "--onlyvisible", "--name",
                                                      "^Notification fixture$"], env=env, text=True).split()[0]
                    key("windowfocus", "--sync", window)
                    self.wait_for(lambda: directory(first), "focused first tab")
                    (base / "focused").touch()
                    shown = self.wait_for(lambda: notices(1), "first notification")[0]
                    # The focused tab's o=unfocused notice never reached the service; text is literal.
                    self.assertEqual((shown["summary"], shown["body"], shown["replaces"]),
                                     ("Fixture <title>", "needs &lt;b&gt;you&lt;/b&gt; now", 0))
                    self.assertEqual((shown["application"], shown["icon"], shown["actions"]),
                                     ("Worminal", "worminal", ["default", "Show"]))
                    self.assertEqual(shown["hints"], {"urgency": 2, "desktop-entry": "worminal"})
                    key("key", "ctrl+t")
                    self.wait_for(lambda: directory(second), "second tab selected")
                    (base / "hidden").touch()
                    replaced = self.wait_for(lambda: notices(2), "background tab notification")[1]
                    self.assertEqual((replaced["summary"], replaced["body"], replaced["replaces"]),
                                     ("second notice", "", shown["id"]))
                    self.assertEqual(replaced["hints"]["urgency"], 1)
                    service.send_signal(signal.SIGUSR1)
                    self.wait_for(lambda: directory(first), "clicked notification selecting its tab")
                    self.assertIsNone(app.poll(), (base / "frontend.log").read_text())
                finally:
                    for process in (app, service):
                        if process.poll() is None:
                            process.terminate()
                        process.wait(timeout=5)
                    stop_workspace(env)


if __name__ == "__main__":
    unittest.main()
