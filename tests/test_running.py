"""The live-process report must see a replaced executable as stale and an untouched one as current."""
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from tools import running
from tools.paths import CHECKS


class RunningTest(unittest.TestCase):
    def test_replaced_executable_is_stale(self):
        CHECKS.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="running-", dir=CHECKS) as directory:
            prefix = Path(directory).resolve()
            (prefix / "bin").mkdir()
            installed = prefix / "bin/worminald"
            shutil.copy2(os.path.realpath(sys.executable), installed)
            child = subprocess.Popen([str(installed), "-c", "import time; time.sleep(60)"])
            try:
                def mine():
                    return [r for r in running.processes(str(prefix)) if r["pid"] == child.pid]
                self.assertTrue(mine()[0]["current"])
                # Atomic replacement, exactly as the installer publishes: the child keeps the old inode.
                shutil.copy2(os.path.realpath(sys.executable), prefix / "bin/new")
                os.replace(prefix / "bin/new", installed)
                record = mine()[0]
                self.assertFalse(record["current"])
                self.assertIn("STALE", running.status(record))
            finally:
                child.kill()
                child.wait()


if __name__ == "__main__":
    unittest.main()
