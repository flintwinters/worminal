"""Resource accounting includes children owned by non-main Linux threads."""

import os
from pathlib import Path
import subprocess
import sys
import threading
import unittest

from perf_metrics import descendants, memory


class PerfMetricsTests(unittest.TestCase):
    def test_worker_owned_child_is_counted_while_worker_is_alive(self):
        ready, release = threading.Event(), threading.Event()
        children, errors = [], []

        def worker():
            child = None
            try:
                child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"],
                                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                children.append(child)
            except Exception as error:
                errors.append(error)
            finally:
                ready.set()
            release.wait(10)
            if child:
                child.terminate()
                child.wait(timeout=5)

        thread = threading.Thread(target=worker, daemon=True)
        thread.start()
        try:
            self.assertTrue(ready.wait(10), "Worker did not start its child")
            if errors:
                raise errors[0]
            child = children[0]
            self.assertIsNone(child.poll())
            main_children = Path(f"/proc/{os.getpid()}/task/{os.getpid()}/children").read_text().split()
            self.assertNotIn(str(child.pid), main_children, "Fixture child must belong to the live worker thread")
            processes = descendants(os.getpid())
            self.assertIn(child.pid, processes)
            self.assertGreaterEqual(memory(processes)["processes"], 2)
        finally:
            release.set()
            thread.join(timeout=10)
        self.assertFalse(thread.is_alive())
