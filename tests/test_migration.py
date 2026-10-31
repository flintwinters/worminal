"""Recovery artifacts must remain independent of subsequent builds and capture failures."""

import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from tools import migration


class BaselineTests(unittest.TestCase):
    def setUp(self):
        checks = migration.ROOT / ".checks"
        checks.mkdir(exist_ok=True)
        directory = tempfile.TemporaryDirectory(prefix="migration-", dir=checks)
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.source = self.root / "worminal"
        self.source.write_bytes(b"baseline")
        self.source.chmod(0o755)
        self.installed = self.root / "installed/worminal"
        self.installed.parent.mkdir()
        self.installed.write_bytes(b"installed")
        root = patch.object(migration, "ROOT", self.root)
        root.start()
        self.addCleanup(root.stop)
        files = patch.object(migration, "installation_files",
                             return_value=[(self.source, self.installed, 0o755)])
        files.start()
        self.addCleanup(files.stop)
        revision = patch.object(migration.subprocess, "check_output", return_value="revision\n")
        revision.start()
        self.addCleanup(revision.stop)

    def test_copies_are_independent_and_never_overwritten(self):
        destination = migration.preserve_baseline()
        manifest = json.loads((destination / "manifest.json").read_text())
        self.assertEqual(manifest["revision"], "revision")
        self.assertEqual(len(manifest["artifacts"]), 2)
        self.source.write_bytes(b"replacement")
        for record in manifest["artifacts"]:
            copy = destination / record["copy"]
            expected = b"baseline" if record["kind"] == "build" else b"installed"
            self.assertEqual(copy.read_bytes(), expected)
            self.assertEqual(record["sha256"], hashlib.sha256(expected).hexdigest())
        self.assertEqual((destination / "build/worminal").stat().st_mode & 0o777, 0o755)
        with self.assertRaises(FileExistsError):
            migration.preserve_baseline()
        self.assertEqual((destination / "build/worminal").read_bytes(), b"baseline")

    def test_failed_capture_cannot_be_mistaken_for_a_complete_baseline(self):
        with patch.object(migration.shutil, "copy2", side_effect=OSError("copy failed")):
            with self.assertRaises(OSError):
                migration.preserve_baseline()
        self.assertFalse((self.root / ".checks/c-baseline").exists())
        migration.preserve_baseline()

    def test_missing_installation_is_explicit(self):
        self.installed.unlink()
        destination = migration.preserve_baseline()
        records = json.loads((destination / "manifest.json").read_text())["artifacts"]
        self.assertEqual(records[1], {"kind": "installed", "path": str(self.installed), "missing": True})
