"""Bounded Python module workers. Unreviewed suites and shared native/standalone GUI phases stay serial."""
from concurrent.futures import ThreadPoolExecutor, as_completed
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import signal
import select
import time
import unittest

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
from tools.paths import CHECKS

# These modules use private temporary resources and do not publish shared build artifacts.
# New or renamed suites stay serial until their resource ownership has been reviewed.
INDEPENDENT = frozenset({'test_theme', 'test_privacy', 'test_perf_metrics', 'test_http_fixture',
                         'test_remote_install', 'test_remote_upgrade', 'test_handoff', 'test_upgrade',
                         'test_window_replacement', 'test_master_startup'})
DISPLAY_ENV = ('DISPLAY', 'WAYLAND_DISPLAY', 'XAUTHORITY', 'DBUS_SESSION_BUS_ADDRESS', 'GDK_BACKEND')


def available_cpus():
    try:
        count = len(os.sched_getaffinity(0))
        if count: return count
    except (AttributeError, OSError, NotImplementedError):
        pass
    for getter in (getattr(os, 'process_cpu_count', None), os.cpu_count):
        if getter is None: continue
        try:
            count = getter()
            if isinstance(count, int) and count > 0: return count
        except (OSError, NotImplementedError):
            pass
    return 1


def default_jobs():
    return available_cpus() // 2 + 1


def command(modules=(), match=None):
    args = [sys.executable, '-m', 'unittest']
    args += list(modules) if modules else ['discover', '-s', str(ROOT/'tests')]
    return [*args, '-q', *(['-k', match] if match else [])]


def environment(directory):
    env = {key: value for key, value in os.environ.items() if key not in DISPLAY_ENV}
    env['WORMINAL_TEST_CHECKS_DIR'] = str(directory)
    env['PYTHONDONTWRITEBYTECODE'] = '1'
    env['PYTHONPATH'] = os.pathsep.join([str(ROOT/'tests'), str(ROOT), env.get('PYTHONPATH', '')])
    env['GIO_USE_VFS'] = 'local'
    for name, child in (('XDG_STATE_HOME', 'state'), ('XDG_CACHE_HOME', 'cache'),
                        ('XDG_DATA_HOME', 'data'), ('XDG_RUNTIME_DIR', 'runtime')):
        path = directory/child
        path.mkdir(mode=0o700)
        env[name] = str(path)
    return env


def retire_worker(process, directory):
    # Give unittest's finally blocks a chance, then retire only this worker's tagged fixtures.
    if process.poll() is None:
        process.send_signal(signal.SIGINT)
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            pass
    marker = b'WORMINAL_TEST_CHECKS_DIR=' + os.fsencode(directory)
    identities = []
    for entry in Path('/proc').iterdir():
        if not entry.name.isdecimal() or int(entry.name) == os.getpid(): continue
        identity = None
        try:
            if entry.stat().st_uid != os.getuid(): continue
            identity = os.pidfd_open(int(entry.name))
            if marker not in (entry/'environ').read_bytes().split(b'\0'): continue
            identities.append(identity)
            identity = None
            signal.pidfd_send_signal(identities[-1], signal.SIGTERM)
        except OSError:
            pass
        finally:
            if identity is not None: os.close(identity)
    try:
        if identities:
            ready, _, _ = select.select(identities, [], [], 2)
            for identity in identities:
                if identity not in ready:
                    try: signal.pidfd_send_signal(identity, signal.SIGKILL)
                    except ProcessLookupError: pass
        if process.poll() is None: process.kill()
        process.wait(timeout=5)
    finally:
        for identity in identities: os.close(identity)


def invoke(name, args, directory, timeout=180):
    directory.mkdir(parents=True)
    started = time.perf_counter()
    log = directory/'output.log'
    with log.open('wb') as output:
        process = subprocess.Popen(args, cwd=ROOT, env=environment(directory), stdout=output, stderr=subprocess.STDOUT)
        try:
            returncode = process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            retire_worker(process, directory)
            output.write(b'\nWorker timed out; tagged private fixtures were retired.\n')
            returncode = 124
        except BaseException:
            retire_worker(process, directory)
            raise
    text = log.read_text(errors='replace')
    counts = re.findall(r'Ran (\d+) tests? in ', text)
    return {'name': name, 'returncode': returncode, 'seconds': time.perf_counter()-started,
            'tests': int(counts[-1]) if counts else 0, 'log': str(log)}


def validate_modules(modules):
    for name in modules:
        if not re.fullmatch(r'test[A-Za-z0-9_]*', name) or not (ROOT/'tests'/f'{name}.py').is_file():
            raise ValueError(f'Unknown top-level test module: {name}')
    if len(set(modules)) != len(modules):
        raise ValueError('Each test module may be selected only once.')


def run_python(jobs=None, match=None, modules=()):
    if jobs is None: jobs = default_jobs()
    if not isinstance(jobs, int) or jobs < 1:
        raise ValueError('Use a positive number of Python workers.')
    validate_modules(modules)
    CHECKS.mkdir(parents=True, exist_ok=True)
    directory = Path(tempfile.mkdtemp(prefix='python-tests-', dir=CHECKS))
    selected = set(modules) if modules else {path.stem for path in (ROOT/'tests').glob('test*.py')}
    parallel = sorted(selected & INDEPENDENT) if jobs > 1 else []
    results = []
    started = time.perf_counter()
    if parallel:
        with ThreadPoolExecutor(max_workers=jobs) as workers:
            pending = {workers.submit(invoke, name, command([name], match), directory/name): name for name in parallel}
            for future in as_completed(pending):
                result = future.result()
                results.append(result)
                print(f'{result["name"]}: {result["tests"]} tests, {result["seconds"]:.2f}s, '
                      f'{"PASS" if result["returncode"] == 0 else "FAIL"}', flush=True)
    remaining = [name for name in modules if name not in parallel] if modules else []
    if not modules or remaining:
        if parallel and not modules:
            args = [sys.executable, str(Path(__file__).resolve()), '--worker',
                    *[item for name in parallel for item in ('--exclude', name)],
                    *(['--match', match] if match else [])]
        else:
            args = command(remaining, match)
        results.append(invoke('serial', args, directory/'serial', timeout=None))
    for result in results:
        # Replay complete output without interleaving concurrent test failures.
        print(Path(result['log']).read_text(errors='replace'), end='', flush=True)
    report = {'jobs': jobs, 'seconds': time.perf_counter()-started, 'results': results,
              'tests': sum(result['tests'] for result in results)}
    (directory/'summary.json').write_text(json.dumps(report, indent=2)+'\n')
    print(f'Python checks: {report["tests"]} tests in {report["seconds"]:.2f}s; logs: {directory}', flush=True)
    return int(any(result['returncode'] for result in results))


class SerialLoader(unittest.TestLoader):
    def __init__(self, excluded):
        super().__init__()
        self.excluded = set(excluded)

    def loadTestsFromModule(self, module, *args, **kwargs):
        if module.__name__ in self.excluded:
            return self.suiteClass()
        return super().loadTestsFromModule(module, *args, **kwargs)


def worker(excluded, match):
    loader = SerialLoader(excluded)
    if match:
        loader.testNamePatterns = [match if '*' in match else f'*{match}*']
    suite = loader.discover(str(ROOT/'tests'))
    result = unittest.TextTestRunner(verbosity=0).run(suite)
    return int(not result.wasSuccessful())


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--worker', action='store_true')
    parser.add_argument('--exclude', action='append', default=[])
    parser.add_argument('--match')
    arguments = parser.parse_args()
    if not arguments.worker:
        parser.error('Use manage.py check-python or check --jobs instead.')
    raise SystemExit(worker(arguments.exclude, arguments.match))
