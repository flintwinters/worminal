"""Help and version must identify the build without launching a graphical session."""

import os
from pathlib import Path
import subprocess
import unittest

from tools.build import VERSION


ROOT = Path(__file__).resolve().parent.parent


class CommandLineTest(unittest.TestCase):
    def test_launcher_does_not_link_graphics_libraries(self):
        dependencies = subprocess.check_output(["ldd", str(ROOT / "worminal")], text=True)
        for name in ("libgtk", "libgdk", "libpango", "libwebkit"):
            self.assertNotIn(name, dependencies)

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
