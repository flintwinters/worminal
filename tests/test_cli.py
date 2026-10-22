"""Help and version must identify the build without launching a graphical session."""

import os
import shutil
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from tools.build import VERSION


ROOT = Path(__file__).resolve().parent.parent


class CommandLineTest(unittest.TestCase):
    def test_browser_command_does_not_require_graphics_or_a_daemon_executable(self):
        checks = ROOT / ".checks"
        checks.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="browser-command-", dir=checks) as directory:
            path = Path(directory)
            shutil.copy2(ROOT / "worminal", path / "worminal")
            env = {**os.environ, "WORMINAL_SHARED_SOCKET_SCOPE": path.name,
                   "DISPLAY": "invalid", "WAYLAND_DISPLAY": "invalid"}
            result = subprocess.run([str(path / "worminal"), "--new-browser-tab"],
                                    env=env, capture_output=True, text=True, timeout=5)
            self.assertEqual(result.returncode, 1)
            self.assertIn("No preview workspace service is running", result.stderr)
            invalid = subprocess.run([str(path / "worminal"), "--new-browser-tab", "one", "two"],
                                     env=env, capture_output=True, text=True, timeout=5)
            self.assertEqual(invalid.returncode, 2)
            self.assertIn("usage:", invalid.stderr)

    def test_launcher_does_not_link_graphics_libraries(self):
        dependencies = subprocess.check_output(["ldd", str(ROOT / "worminal")], text=True)
        for name in ("libgtk", "libgdk", "libpango", "libwebkit"):
            self.assertNotIn(name, dependencies)

    def test_browser_resolves_against_frontend_module_boundary(self):
        script = """
import ctypes, os, sys
frontend = ctypes.CDLL(sys.argv[1], mode=os.RTLD_NOW | os.RTLD_LOCAL)
browser = ctypes.CDLL(sys.argv[2], mode=os.RTLD_NOW)
assert frontend.worminal_frontend_v1 and browser.worminal_browser_module
for name in ('app_send', 'wire_put_string'):
    try:
        getattr(frontend, name)
    except AttributeError:
        pass
    else:
        raise AssertionError('frontend leaks browser callbacks: ' + name)
try:
    frontend.tsessionnew
except AttributeError:
    pass
else:
    raise AssertionError('frontend exports the owner/parser constructor')
"""
        result = subprocess.run([sys.executable, "-c", script, str(ROOT / "build/frontend.so"),
                                 str(ROOT / "build/browser.so")], capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)

    def invoke(self, option):
        env = {key: value for key, value in os.environ.items()
               if key not in ("DISPLAY", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS")}
        env["GDK_BACKEND"] = "x11"
        result = subprocess.run([str(ROOT / "worminal"), option], env=env,
                                capture_output=True, text=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stderr, "")
        return result.stdout

    def test_help_without_a_display(self):
        for option in ("-h", "--help"):
            with self.subTest(option=option):
                output = self.invoke(option)
                self.assertEqual(output.splitlines()[0], f"worminal {VERSION}")
                self.assertIn("usage: worminal", output)
                self.assertIn("--help", output)
                self.assertIn("--version", output)

    def test_version_without_a_display(self):
        for option in ("-v", "--version"):
            with self.subTest(option=option):
                self.assertEqual(self.invoke(option), f"worminal {VERSION}\n")
