"""Private fixture lifetimes: stop scoped owners before deleting executable runtime files."""
from contextlib import ExitStack, nullcontext
import os
from pathlib import Path
import select
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

from runtime import CHECKS, runtime_binary, stop_application
from workspace_protocol import service_pid, stop_workspace
from x11 import isolated_display


class RuntimeCleanupTest(unittest.TestCase):
    def start(self, directory, env, version, cleanup):
        executable = Path(directory) / ('owner-v' + str(version))
        shutil.copy2(runtime_binary('worminald' if version == 2 else 'worminald-x11'), executable)
        env['XDG_STATE_HOME'] = str(Path(directory) / 'state')
        process = subprocess.Popen([str(executable), '--serve'], env=env,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        cleanup.callback(stop_application, process)
        endpoint = 'worminal-workspace-v2' if version == 2 else 'worminal-workspace'
        deadline = time.monotonic() + 5
        while True:
            with socket.socket(socket.AF_UNIX) as peer:
                peer.settimeout(1)
                try:
                    peer.connect(f"\0{endpoint}-{os.getuid()}-{env['WORMINAL_SHARED_SOCKET_SCOPE']}")
                except ConnectionRefusedError:
                    self.assertIsNone(process.poll(), 'private owner exited before listening')
                    self.assertLess(time.monotonic(), deadline, 'private owner did not listen')
                    time.sleep(.01)
                    continue
                self.assertEqual(service_pid(peer, version), process.pid)
                break
        identity = os.pidfd_open(process.pid)
        cleanup.callback(os.close, identity)
        return process, identity

    def test_display_teardown_awaits_both_protocol_owners_before_runtime_deletion(self):
        class BodyFailure(Exception):
            pass

        for fail in (False, True):
            with self.subTest(body_failure=fail):
                # Keep fallback process cleanup inside the directory lifetime even when the tested
                # display cleanup is broken. This regression must fail on a live owner, not on rmtree.
                with (tempfile.TemporaryDirectory(prefix='runtime-cleanup-', dir=CHECKS) as directory,
                      ExitStack() as cleanup):
                    owners = []
                    expected = self.assertRaises(BodyFailure) if fail else nullcontext()
                    with expected:
                        with isolated_display() as env:
                            for version in (1, 2):
                                owners.append(self.start(directory, env, version, cleanup))
                            if fail:
                                raise BodyFailure('exercise exceptional context unwinding')
                    for process, identity in owners:
                        self.assertTrue(select.select([identity], [], [], 0)[0],
                                        'display teardown returned while a fixture owner still mapped its executable')
                        self.assertEqual(process.wait(timeout=1), 0)
                    self.assertTrue(Path(directory).exists(), 'owner shutdown must precede runtime removal')
                self.assertFalse(Path(directory).exists())

    def test_stop_requires_a_scope_and_an_explicit_supported_protocol(self):
        for env, version in (({}, 2), ({'WORMINAL_SHARED_SOCKET_SCOPE': ''}, 2),
                             ({'WORMINAL_SHARED_SOCKET_SCOPE': 'test-only'}, 3)):
            with self.subTest(env=env, version=version), self.assertRaises(ValueError):
                stop_workspace(env, version)

    def test_stop_waits_for_exit_and_reports_a_stalled_owner(self):
        with (tempfile.TemporaryDirectory(prefix='runtime-stop-', dir=CHECKS) as directory,
              ExitStack() as cleanup, isolated_display() as env):
            process, identity = self.start(directory, env, 2, cleanup)

            def paused_owner(peer, version):
                pid = service_pid(peer, version)
                os.kill(process.pid, signal.SIGSTOP)
                return pid

            try:
                with patch('workspace_protocol.service_pid', side_effect=paused_owner):
                    with self.assertRaisesRegex(TimeoutError, 'did not stop'):
                        stop_workspace(env, timeout=.1)
                self.assertFalse(select.select([identity], [], [], 0)[0])
            finally:
                os.kill(process.pid, signal.SIGCONT)
            stop_workspace(env)
            self.assertTrue(select.select([identity], [], [], 0)[0])


def prove_cleanup():
    test = RuntimeCleanupTest('test_display_teardown_awaits_both_protocol_owners_before_runtime_deletion')
    log_path = CHECKS / 'runtime-cleanup-proof.log'
    with log_path.open('w') as log, patch('x11.stop_workspace'):
        result = unittest.TextTestRunner(stream=log, verbosity=2).run(unittest.TestSuite([test]))
    if (result.errors or len(result.failures) != 2 or
            any('display teardown returned while a fixture owner still mapped its executable' not in detail
                for _, detail in result.failures)):
        raise AssertionError(f'Regression did not detect omitted owner cleanup; see {log_path}')
    print(f'Cleanup regression detects missing owner retirement on both exit paths: {log_path}')


if __name__ == '__main__':
    if sys.argv[1:] == ['--prove']:
        prove_cleanup()
    else:
        unittest.main()
