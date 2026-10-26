"""Resource accounting and pixel sampling for performance checks."""

import os
from pathlib import Path
import select
import subprocess
import sys
import threading
import unittest
from types import SimpleNamespace
from unittest.mock import Mock

from perf_metrics import descendants, memory, startup_usage
from visible_latency import ready


class PerfMetricsTests(unittest.TestCase):
    def test_binary_and_utf8_decoded_pixels_are_sampled_without_corruption(self):
        window = Mock()
        window.get_geometry.return_value = SimpleNamespace(width=400, height=300)
        for payload in ("", b""):
            window.get_image.return_value.data = payload
            self.assertFalse(ready(window))
        for color in (0x112233, 0x00a9c2):
            pixel = color.to_bytes(4, sys.byteorder)
            for payload in (pixel, pixel.decode("utf-8")):
                window.get_image.return_value.data = payload
                self.assertTrue(ready(window, color))

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


    def test_rss_high_water_survives_released_allocation(self):
        code = """import gc, sys
payload = bytearray(32 * 1024 * 1024)
for offset in range(0, len(payload), 4096): payload[offset] = 1
print('allocated', flush=True)
sys.stdin.readline()
del payload
gc.collect()
print('released', flush=True)
sys.stdin.readline()
"""
        child = subprocess.Popen([sys.executable, "-c", code], stdin=subprocess.PIPE,
                                 stdout=subprocess.PIPE, text=True)
        try:
            def stage(expected):
                self.assertTrue(select.select([child.stdout], [], [], 5)[0])
                self.assertEqual(child.stdout.readline().strip(), expected)

            stage("allocated")
            before = startup_usage(child.pid)["processes"][0]
            child.stdin.write("release\n"); child.stdin.flush()
            stage("released")
            after = startup_usage(child.pid)["processes"][0]
            self.assertEqual(after["pid"], child.pid)
            # Kernel RSS counters are approximate; verify the large allocation
            # survives as a high-water observation without asserting exact ordering.
            self.assertGreater(before["rss_kib"], 24 * 1024)
            self.assertGreater(after["approx_rss_peak_kib"], 24 * 1024)
            self.assertGreater(after["approx_rss_peak_kib"] - after["rss_kib"], 16 * 1024)
            self.assertGreaterEqual(after["cpu_ms"], before["cpu_ms"])
            self.assertGreaterEqual(after["main_thread_cpu_ms"], before["main_thread_cpu_ms"])
        finally:
            child.terminate()
            child.wait(timeout=5)
            child.stdin.close(); child.stdout.close()
