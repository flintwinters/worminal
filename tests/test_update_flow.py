"""Guards that keep checked updates from failing for environmental reasons."""
import os
import subprocess
import sys
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from tools import auto_update
from tools.paths import CHECKS, ROOT, socket_directory
from tools.update_storage import release_stale


class UpdateFlowTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(dir=CHECKS, prefix="update-flow-")
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)

    def test_private_workers_share_prepared_muffin_runtime(self):
        worker = self.root / "worker"
        worker.mkdir()
        result = subprocess.check_output(
            [sys.executable, "-c", "from tools.muffin import LOCAL; print(LOCAL)"],
            cwd=ROOT, env={**os.environ, "WORMINAL_TEST_CHECKS_DIR": str(worker)}, text=True)
        self.assertEqual(Path(result.strip()), ROOT / "build/dependencies/muffin")

    def test_socket_fixture_binds_outside_nested_worker_diagnostics(self):
        import socket
        with socket_directory() as directory, socket.socket(socket.AF_UNIX) as listener:
            address = Path(directory) / "worminal-test"
            listener.bind(str(address))
            self.assertFalse(address.is_relative_to(CHECKS))
        self.assertFalse(Path(directory).exists())

    def test_second_update_of_a_checkout_is_refused_not_interleaved(self):
        with auto_update.exclusive(self.root):
            with self.assertRaises(SystemExit) as refused:
                with auto_update.exclusive(self.root):
                    self.fail("entered while held")
            self.assertIn("Another update", str(refused.exception))
        with auto_update.exclusive(self.root):
            pass

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
