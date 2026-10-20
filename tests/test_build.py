"""Build ownership: dependency invalidation, failed compiles, staging, and cleanup."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from tools import build


class BuildTests(unittest.TestCase):
    def setUp(self):
        checks = build.ROOT / ".checks"
        checks.mkdir(exist_ok=True)
        self.directory = tempfile.TemporaryDirectory(prefix="build-", dir=checks)
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.root_patch = patch.object(build, "ROOT", self.root)
        self.root_patch.start()
        self.addCleanup(self.root_patch.stop)

    def test_release_and_rollback_own_matching_protocols(self):
        with patch.object(build, "pkg_flags", return_value=[]):
            builder = build.Builder()
            with patch.object(builder, "program") as program:
                for name in ("worminal", "worminald", "worminal-preview", "worminald-preview"):
                    builder.target(name)
                    args, kwargs = program.call_args
                    self.assertIn("-DWORMINAL_PREVIEW_PROTOCOL", kwargs["flags"])
                    self.assertEqual("src/gtk/app.c" in args[1], name in ("worminal", "worminal-preview"))
                for name in ("worminal-x11", "worminald-x11"):
                    builder.target(name)
                    args, kwargs = program.call_args
                    self.assertNotIn("-DWORMINAL_PREVIEW_PROTOCOL", kwargs["flags"])
                    self.assertIn('-DWORKSPACE_DAEMON="worminald-x11"', kwargs["flags"])

    def test_defaults_and_environment_flags(self):
        with patch.dict(os.environ, {}, clear=True), patch.object(build, "pkg_flags", return_value=[]):
            self.assertIn("-O3", build.compile_flags())
            with patch.dict(os.environ, {"CFLAGS": "-O0 -g", "CPPFLAGS": '-DNAME="two words"'}):
                flags = build.compile_flags()
                self.assertNotIn("-O3", flags)
                self.assertIn("-O0", flags)
                self.assertIn("-DNAME=two words", flags)

    def test_incremental_headers_flags_and_failure_recovery(self):
        source = self.root / "source.c"
        header = self.root / "value.h"
        source.write_text('#include "value.h"\nint main(void) { return VALUE; }\n')
        header.write_text("#define VALUE 0\n")
        command = ["cc", "-c", "source.c"]
        with patch.object(build, "invoke", wraps=build.invoke) as invoked:
            build.step("output.o", command, ["source.c"], depfile=True)
            output = self.root / "output.o"
            original = output.read_bytes()
            build.step("output.o", command, ["source.c"], depfile=True)
            self.assertEqual(invoked.call_count, 1)
            # Set an explicit later timestamp so the regression does not rely on clock resolution.
            header.write_text("#define VALUE 1\n")
            later = output.stat().st_mtime_ns + 1_000_000
            os.utime(header, ns=(later, later))
            build.step("output.o", command, ["source.c"], depfile=True)
            self.assertEqual(invoked.call_count, 2)
            os.utime(header, ns=(1, 1))
            build.step("output.o", [*command, "-O2"], ["source.c"], depfile=True)
            self.assertEqual(invoked.call_count, 3)
            good = output.read_bytes()
            self.assertNotEqual(original, good)
            with patch.object(build, "invoke", side_effect=subprocess.CalledProcessError(1, command)):
                with self.assertRaises(subprocess.CalledProcessError):
                    build.step("output.o", [*command, "-invalid"], ["source.c"], depfile=True)
            self.assertEqual(output.read_bytes(), good)
            self.assertFalse(list(self.root.glob("*.pending")))
            build.step("output.o", [*command, "-O2"], ["source.c"], depfile=True)
            self.assertEqual(invoked.call_count, 3)

    def test_staged_install_uninstall_and_modes(self):
        for source in ("worminal", "worminald", "worminal-x11", "worminald-x11", "docs/man/worminal.1",
                       "assets/worminal.desktop", "assets/worminal.svg"):
            path = self.root / source
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("VERSION\n")
        stage = self.root / "stage"
        with patch.dict(os.environ, {"DESTDIR": str(stage), "MANPREFIX": "/manuals"}), \
                patch.object(build, "build_targets") as compile_build:
            build.install("/opt/worminal")
            compile_build.assert_called_once_with("worminal-x11", "worminald-x11")
            files = build.installation_files("/opt/worminal")
            for _, destination, mode in files:
                self.assertTrue(destination.is_relative_to(stage))
                self.assertEqual(destination.stat().st_mode & 0o777, mode)
            self.assertEqual((stage / "manuals/man1/worminal.1").read_text(), build.VERSION + "\n")
            keep = stage / "keep"
            keep.touch()
            build.uninstall("/opt/worminal")
            self.assertTrue(keep.exists())
            self.assertTrue(all(not destination.exists() for _, destination, _ in files))

    def test_clean_preserves_diagnostics(self):
        (self.root / "build").mkdir()
        (self.root / "build/object.o").touch()
        (self.root / ".checks").mkdir()
        (self.root / ".checks/failure.log").touch()
        for name in ("worminal", "worminald", "worminal.command.json"):
            (self.root / name).touch()
        build.clean()
        self.assertFalse((self.root / "build").exists())
        self.assertFalse((self.root / "worminal").exists())
        self.assertFalse((self.root / "worminal.command.json").exists())
        self.assertTrue((self.root / ".checks/failure.log").exists())

    def test_relative_install_prefix_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "absolute"):
            build.installation_files("relative")
