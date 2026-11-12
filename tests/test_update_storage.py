"""Update verification storage preserves checks and diagnostics on UID-remapping filesystems."""
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from tools.paths import CHECKS
from tools.update_storage import verification_environment


class StorageTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(dir=CHECKS, prefix="update-storage-")
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.uid = self.root.stat().st_uid

    def test_matching_ownership_needs_no_mount_and_tags_private_verification(self):
        with patch("tools.update_storage.os.geteuid", return_value=self.uid), patch(
                "tools.update_storage.subprocess.run") as run:
            with verification_environment(self.root) as env:
                path = Path(env.pop("WORMINAL_TEST_CHECKS_DIR"))
                self.assertTrue(path.is_relative_to(self.root / "build/checks"))
                self.assertEqual(env, {key: value for key, value in os.environ.items()
                                       if key != "WORMINAL_TEST_CHECKS_DIR"})
        run.assert_not_called()
        self.assertTrue(path.is_dir())

    def test_remapped_storage_mounts_and_preserves_diagnostics_before_unmount(self):
        def command(args, **kwargs):
            if "umount" in args:
                self.assertEqual(next((self.root / "build/checks").glob("*-saved/result.log")).read_text(),
                                 "diagnostic")
                # A real unmount reveals the empty mountpoint beneath the tmpfs.
                (Path(args[-1]) / "result.log").unlink()
        with patch("tools.update_storage.os.geteuid", return_value=self.uid + 1), patch(
                "tools.update_storage.subprocess.run", side_effect=command) as run:
            with verification_environment(self.root) as env:
                path = Path(env["WORMINAL_TEST_CHECKS_DIR"])
                self.assertTrue(path.is_relative_to(self.root / "build/checks"))
                (path / "result.log").write_text("diagnostic")
        self.assertEqual([call.args[0][2] for call in run.call_args_list], ["mount", "umount"])
        self.assertFalse(path.exists())


if __name__ == "__main__":
    unittest.main()
