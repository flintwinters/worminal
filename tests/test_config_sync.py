"""worminald config sync between two private nodes, each with its own HOME, joined by a fake ssh."""
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

from runtime import CHECKS
from tools.build import Builder

FAKE_SSH = """#!/usr/bin/env python3
import os, sys
if sys.argv[1:6] != ["-T", "-C", "-oConnectTimeout=10", "peer", "worminald"] or sys.argv[6:] != ["config", "exchange"]:
    sys.exit(64)
os.environ["WORMINAL_CONFIG"] = os.environ["WORMINAL_REMOTE_CONFIG"]
os.execv(os.environ["WORMINAL_TEST_DAEMON"], [os.environ["WORMINAL_TEST_DAEMON"], "config", "exchange"])
"""


class ConfigSyncTest(unittest.TestCase):
    def setUp(self):
        self.daemon = str(Builder().rust_artifact("worminal-daemon"))
        directory = tempfile.TemporaryDirectory(prefix="config-sync-", dir=CHECKS)
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        (root / "bin").mkdir()
        ssh = root / "bin/ssh"
        ssh.write_text(FAKE_SSH)
        ssh.chmod(0o755)
        self.local, self.remote = root / "local", root / "remote"
        self.local_file, self.remote_file = self.local / "worminal.yaml", self.remote / "worminal.yaml"
        for node in (self.local, self.remote):
            node.mkdir()
        self.path = f"{root / 'bin'}:{os.environ['PATH']}"
        self.state = root / "state"

    def sync(self, *extra):
        # The fake ssh switches WORMINAL_CONFIG to the remote file, as a different account would see its own.
        env = {**os.environ, "PATH": self.path, "WORMINAL_TEST_DAEMON": self.daemon,
               "WORMINAL_CONFIG": str(self.local_file), "XDG_STATE_HOME": str(self.state),
               "WORMINAL_REMOTE_CONFIG": str(self.remote_file)}
        result = subprocess.run([self.daemon, "config", "sync", *extra, "peer"], env=env,
                                capture_output=True, text=True, timeout=30)
        return result, json.loads(result.stdout)

    def test_independent_edits_merge_and_conflicts_wait_for_a_preference(self):
        self.local_file.write_text("font:\n  size: 9\nscrolling:\n  history: 100\n")
        self.remote_file.write_text("font:\n  size: 9\nscrolling:\n  history: 100\n")
        result, report = self.sync()
        self.assertEqual((result.returncode, report["conflicts"]), (0, []), result.stderr)
        self.local_file.write_text("font:\n  size: 12\nscrolling:\n  history: 100\n")
        self.remote_file.write_text("font:\n  size: 9\nscrolling:\n  history: 500\n")
        result, report = self.sync()
        self.assertEqual(report["conflicts"], [], result.stderr)
        merged = "font:\n  size: 12\nscrolling:\n  history: 500\n"
        self.assertEqual((self.local_file.read_text(), self.remote_file.read_text()), (merged, merged))
        self.local_file.write_text(merged.replace("size: 12", "size: 13"))
        self.remote_file.write_text(merged.replace("size: 12", "size: 14"))
        result, report = self.sync()
        self.assertEqual((report["conflicts"], report["changed_local"], report["changed_remote"]),
                         (["font.size"], False, False))
        self.assertIn("size: 13", self.local_file.read_text())
        self.assertIn("size: 14", self.remote_file.read_text())
        result, report = self.sync("--prefer", "remote")
        self.assertEqual(report["conflicts"], [])
        self.assertIn("size: 14", self.local_file.read_text())

    def test_invalid_peer_result_is_refused_and_leaves_both_files(self):
        self.local_file.write_text("scrolling:\n  history: 999999\n")
        before = self.local_file.read_text()
        result, report = self.sync()
        self.assertFalse(report["ok"])
        self.assertEqual(self.local_file.read_text(), before)
        self.assertFalse(self.remote_file.exists())


if __name__ == "__main__":
    unittest.main()
