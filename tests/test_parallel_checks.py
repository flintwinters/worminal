"""Process isolation, overlap, serial barriers, filtering and failure propagation for Python workers."""
from contextlib import redirect_stdout
import io
import json
import os
from pathlib import Path
import tempfile
import sys
import subprocess
import unittest
from unittest.mock import patch

from runtime import CHECKS
from tools import test_runner


class ParallelChecksTest(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix='parallel-runner-', dir=CHECKS)
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        (self.root/'tests').mkdir()
        for name, value in (('ROOT', self.root), ('CHECKS', self.root/'build/checks')):
            context = patch.object(test_runner, name, value)
            context.start()
            self.addCleanup(context.stop)
        context = patch.object(test_runner, 'INDEPENDENT', frozenset({'test_a', 'test_b'}))
        context.start()
        self.addCleanup(context.stop)

    def module(self, name, body):
        (self.root/'tests'/f'{name}.py').write_text(
            'import unittest,os,time,json,pathlib\nclass Fixture(unittest.TestCase):\n'+body)

    def run_checks(self, jobs, modules, match=None):
        with redirect_stdout(io.StringIO()):
            code = test_runner.run_python(jobs=jobs, modules=modules, match=match)
        summaries = sorted((self.root/'build/checks').glob('python-tests-*/summary.json'),
                           key=lambda p:p.stat().st_mtime_ns)
        return code, json.loads(summaries[-1].read_text())

    def test_workers_overlap_and_serial_tail_waits_with_private_environment(self):
        events = self.root/'events'
        for name in ('test_a', 'test_b'):
            self.module(name, f'''    def test_overlap(self):
        root=pathlib.Path({str(events)!r}); root.mkdir(exist_ok=True)
        (root/{name!r}).write_text(json.dumps({{"checks":os.environ["WORMINAL_TEST_CHECKS_DIR"],
            "display":os.environ.get("DISPLAY"),"state":os.environ["XDG_STATE_HOME"]}}))
        deadline=time.monotonic()+5
        while not all((root/name).exists() for name in ("test_a","test_b")):
            self.assertLess(time.monotonic(),deadline,"Workers did not overlap")
            time.sleep(.01)
        (root/({name!r}+"-done")).touch()
''')
        self.module('test_serial', f'''    def test_after_workers(self):
        root=pathlib.Path({str(events)!r})
        self.assertTrue(all((root/(name+"-done")).exists() for name in ("test_a","test_b")))
''')
        with patch.dict(os.environ, {'DISPLAY': ':never-use', 'DBUS_SESSION_BUS_ADDRESS': 'never-use'}):
            code, report = self.run_checks(2, ['test_a', 'test_b', 'test_serial'])
        self.assertEqual(code, 0)
        self.assertEqual(report['tests'], 3)
        rows = [json.loads((events/name).read_text()) for name in ('test_a', 'test_b')]
        self.assertNotEqual(rows[0]['checks'], rows[1]['checks'])
        self.assertNotEqual(rows[0]['state'], rows[1]['state'])
        self.assertTrue(all(row['display'] is None for row in rows))

    def test_filter_and_counts_match_serial_and_parallel(self):
        for name in ('test_a', 'test_b'):
            self.module(name, '    def test_keep(self): pass\n    def test_drop(self): self.fail("Filtered out")\n')
        for jobs in (1, 2):
            code, report = self.run_checks(jobs, ['test_a', 'test_b'], match='keep')
            self.assertEqual(code, 0)
            self.assertEqual(report['tests'], 2)

    def test_failure_is_reported_and_remaining_tests_still_run(self):
        self.module('test_a', '    def test_fail(self): self.fail("Expected fixture failure")\n')
        self.module('test_serial', '    def test_pass(self): pass\n')
        code, report = self.run_checks(2, ['test_a', 'test_serial'])
        self.assertEqual(code, 1)
        self.assertEqual(report['tests'], 2)
        self.assertTrue(any('Expected fixture failure' in Path(row['log']).read_text() for row in report['results']))

    def test_discovery_keeps_unreviewed_modules_after_excluding_worker_modules(self):
        for name in ('test_a', 'test_b'):
            self.module(name, '    def test_one(self): pass\n    def test_two(self): pass\n')
            self.addCleanup(sys.modules.pop, name, None)
        suite = test_runner.SerialLoader(['test_a']).discover(str(self.root/'tests'))
        self.assertEqual(suite.countTestCases(), 2)

    def test_worker_diagnostics_cannot_escape_project_checks(self):
        env = {**os.environ, 'WORMINAL_TEST_CHECKS_DIR': '/tmp/not-a-project-check'}
        result = subprocess.run([sys.executable, '-c', 'from tools.paths import CHECKS'],
                                cwd=Path(__file__).resolve().parent.parent, env=env, capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('below project build/checks', result.stderr)

    def test_finished_and_timed_out_workers_retire_their_tagged_children(self):
        marker = self.root/'child-pid'
        script = self.root/'hang.py'
        script.write_text('import subprocess,sys,time,pathlib\n'
                          'child=subprocess.Popen([sys.executable,"-c","import time;time.sleep(30)"])\n'
                          f'pathlib.Path({str(marker)!r}).write_text(str(child.pid))\n'
                          'time.sleep(30)\n')
        for name, timeout, delay, expected in (('timeout', .2, 30, 124), ('finished', None, 0, 0)):
            with self.subTest(name=name):
                source = script.read_text().replace('time.sleep(30)\n', f'time.sleep({delay})\n')
                script.write_text(source)
                result = test_runner.invoke('fixture', [sys.executable,str(script)], self.root/name, timeout=timeout)
                self.assertEqual(result['returncode'], expected)
                process = Path(f'/proc/{marker.read_text()}/stat')
                try:
                    state = process.read_text().rsplit(')',1)[1].split()[0]
                except (FileNotFoundError, ProcessLookupError):
                    continue  # The child can be reaped between opening and reading /proc.
                self.assertEqual(state, 'Z')

    def test_automatic_default_and_explicit_override(self):
        self.module('test_a', '    def test_pass(self): pass\n')
        with patch.object(test_runner, 'default_jobs', return_value=17) as default:
            code, report = self.run_checks(None, ['test_a'])
            self.assertEqual((code, report['jobs']), (0, 17))
            default.assert_called_once_with()
        with patch.object(test_runner, 'default_jobs', side_effect=AssertionError('Override ignored')):
            code, report = self.run_checks(1, ['test_a'])
            self.assertEqual((code, report['jobs']), (0, 1))

    def test_unknown_and_duplicate_modules_are_refused(self):
        self.module('test_a', '    def test_pass(self): pass\n')
        for modules in (['../outside'], ['test_a', 'test_a']):
            with self.assertRaises(ValueError): test_runner.validate_modules(modules)


class CpuDefaultTest(unittest.TestCase):
    def test_affinity_controls_formula_including_counts_above_old_cap(self):
        for cores, expected in ((1,1),(2,2),(3,2),(12,7),(32,17)):
            with self.subTest(cores=cores), patch.object(test_runner.os,'sched_getaffinity',
                                                       return_value=set(range(cores)),create=True):
                self.assertEqual(test_runner.default_jobs(), expected)

    def test_portable_process_count_fallback(self):
        with patch.object(test_runner.os,'sched_getaffinity',side_effect=OSError,create=True), \
             patch.object(test_runner.os,'process_cpu_count',return_value=6,create=True):
            self.assertEqual(test_runner.default_jobs(),4)

    def test_unknown_or_invalid_counts_fall_back_to_one(self):
        for count in (None,0,-1):
            with self.subTest(count=count), \
                 patch.object(test_runner.os,'sched_getaffinity',side_effect=NotImplementedError,create=True), \
                 patch.object(test_runner.os,'process_cpu_count',return_value=None,create=True), \
                 patch.object(test_runner.os,'cpu_count',return_value=count):
                self.assertEqual(test_runner.default_jobs(),1)

    def test_system_count_used_when_process_count_unavailable(self):
        with patch.object(test_runner.os,'sched_getaffinity',return_value=set(),create=True), \
             patch.object(test_runner.os,'process_cpu_count',None,create=True), \
             patch.object(test_runner.os,'cpu_count',return_value=8):
            self.assertEqual(test_runner.default_jobs(),5)


if __name__ == '__main__':
    unittest.main()
