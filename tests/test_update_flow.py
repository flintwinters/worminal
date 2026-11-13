"""Guards that keep checked updates from failing for environmental reasons."""
import os
from pathlib import Path
import tempfile
import time
import unittest
from unittest.mock import patch

from tools import auto_update, explorer
from tools.paths import CHECKS
from tools.update_storage import release_stale


class UpdateFlowTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(dir=CHECKS, prefix="update-flow-")
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)

    def test_second_update_of_a_checkout_is_refused_not_interleaved(self):
        with auto_update.exclusive(self.root):
            with self.assertRaises(SystemExit) as refused:
                with auto_update.exclusive(self.root):
                    self.fail("entered while held")
            self.assertIn("Another update", str(refused.exception))
        with auto_update.exclusive(self.root):
            pass

    def test_explorer_dist_older_than_sources_is_stale(self):
        source = self.root / "apps/explorer/src/main.ts"
        source.parent.mkdir(parents=True)
        source.write_text("x")
        dist = self.root / "dist/index.html"
        dist.parent.mkdir()
        with patch.object(explorer, "EXPLORER", self.root), patch.object(explorer, "DIST", dist):
            self.assertTrue(explorer.dist_stale())  # missing
            dist.write_text("y")
            now = time.time()
            os.utime(source, (now - 10, now - 10))
            os.utime(dist, (now, now))
            self.assertFalse(explorer.dist_stale())
            os.utime(source, (now + 10, now + 10))
            self.assertTrue(explorer.dist_stale())

    def test_only_mounts_of_aborted_updates_are_released(self):
        checks = (self.root / "build/checks").resolve()
        stale, saved, other = (checks / name for name in ("update-posix-abc", "update-posix-abc-saved", "keep"))
        for path in (stale, saved, other):
            path.mkdir(parents=True)
        mounts = "".join(f"tmpfs {path} tmpfs rw 0 0\n" for path in (stale, saved, other))
        with patch("tools.update_storage.Path.read_text", return_value=mounts), \
                patch("tools.update_storage.subprocess.run") as run:
            release_stale(self.root)
        self.assertEqual([call.args[0][-1] for call in run.call_args_list], [str(stale)])


if __name__ == "__main__":
    unittest.main()
