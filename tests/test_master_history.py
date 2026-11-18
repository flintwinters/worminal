"""Native master discovery over private real bootstrap/bridge connections, without a desktop or live SSH."""
import json
import shutil
import socket
import struct
import subprocess
import time
import unittest

import test_remote_install as bootstrap
from workspace_protocol import send, wait_kind

DAEMON = bootstrap.DAEMON


class MasterHistoryTest(unittest.TestCase):
    def setUp(self):
        self.fixture = bootstrap.RemoteCommandTest(methodName='runTest')
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.root = self.fixture.root
        self.env = self.fixture.connect_environment()
        self.fixture.fake(self.fixture.prefix, '0.0.1')
        shutil.copyfile(DAEMON, self.fixture.prefix/'bin/worminald')

    def listing(self, env=None):
        result = subprocess.run([str(DAEMON), '--list-masters'], env=env or self.env,
                                capture_output=True, text=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        return json.loads(result.stdout)['masters']

    def connect(self, endpoint='fixture-host', **environment):
        peer, child = socket.socketpair()
        peer.settimeout(10)
        with child:
            process = subprocess.Popen([str(DAEMON), '--connect-master', endpoint, '--bridge'],
                                       env={**self.env, **environment}, stdin=child, stdout=child,
                                       stderr=subprocess.PIPE)
        def cleanup():
            peer.close()
            try:
                process.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.communicate(timeout=5)
        self.addCleanup(cleanup)
        return peer, process

    def catalog(self, peer):
        send(peer, 19, version=2)
        wait_kind(peer, 7, version=2)

    def test_empty_listing_and_help_never_create_state_start_a_daemon_or_run_ssh(self):
        self.assertEqual(self.listing(), [])
        for arguments in (['--help'], ['--list-masters', '--help']):
            result = subprocess.run([str(DAEMON), *arguments], env=self.env,
                                    capture_output=True, text=True, timeout=5)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn('--list-masters', result.stdout)
        self.assertFalse((self.root/'state').exists())
        self.assertFalse((self.root/'ssh-trace').exists())
        invalid = subprocess.run([str(DAEMON), '--list-masters', 'extra'], env=self.env,
                                 capture_output=True, text=True, timeout=5)
        self.assertEqual(invalid.returncode, 2)
        self.assertFalse(json.loads(invalid.stdout)['ok'])
        self.assertFalse((self.root/'state').exists())

    def test_catalog_commit_multiple_views_crash_cleanup_and_persistent_history(self):
        peer, first = self.connect()
        # A bridge can exist before its first catalog. Merely reaching the service is not
        # proof that a view connection succeeded; the discovery command itself stays passive.
        deadline = time.monotonic()+10
        while True:
            probe = subprocess.run([str(DAEMON), 'api', 'doctor', '--json'], env=self.env,
                                   capture_output=True, text=True, timeout=3)
            if probe.returncode == 0:
                break
            self.assertIsNone(first.poll())
            self.assertLess(time.monotonic(), deadline, probe.stderr + probe.stdout)
            time.sleep(.02)
        self.assertEqual(self.listing(), [])
        self.catalog(peer)
        expected = [{'endpoint': 'fixture-host', 'connected': True}]
        self.assertEqual(self.listing(), expected)
        self.assertEqual(self.listing({**self.env, 'WORMINAL_SHARED_SOCKET_SCOPE': 'other-scope'}), expected)
        second_peer, second = self.connect()
        self.catalog(second_peer)
        self.assertEqual(self.listing(), expected)
        peer.close()
        first.communicate(timeout=5)
        self.assertEqual(self.listing(), expected)
        second.kill()  # Kernel leases must release even when no destructor runs.
        second.communicate(timeout=5)
        expected[0]['connected'] = False
        self.assertEqual(self.listing(), expected)
        self.assertEqual(self.listing({**self.env, 'WORMINAL_SHARED_SOCKET_SCOPE': 'different-scope'}), expected)
        self.assertEqual(self.listing({**self.env, 'XDG_STATE_HOME': str(self.root/'different-state')}), [])
        another, _ = self.connect('alias-for-same-host')
        self.catalog(another)
        self.assertEqual(self.listing(), [
            {'endpoint': 'alias-for-same-host', 'connected': True}, *expected])

    def test_failed_authentication_and_invalid_catalogs_are_not_history(self):
        result = subprocess.run([str(DAEMON), '--connect-master', 'never-connected', '--bridge'],
                                env={**self.env, 'WORMINAL_TEST_INSTALL_SCENARIO': 'missing-python'},
                                input=b'', capture_output=True, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.listing(), [])
        malformed = struct.pack('!4I', 2, 7, 0, 3) + b'bad'
        peer, process = self.connect('bad-catalog', WORMINAL_TEST_MASTER_REPLY=malformed.hex())
        try:
            while peer.recv(4096):
                pass
        except ConnectionResetError:
            pass
        process.communicate(timeout=5)
        self.assertEqual(self.listing(), [])

    def test_corrupt_history_returns_a_structured_error_without_rewriting_it(self):
        directory = self.root/'state/worminal/masters'
        directory.mkdir(parents=True)
        record = directory/'host'
        record.write_text('preserve')
        result = subprocess.run([str(DAEMON), '--list-masters'], env=self.env,
                                capture_output=True, text=True, timeout=5)
        self.assertEqual(result.returncode, 1)
        self.assertFalse(json.loads(result.stdout)['ok'])
        self.assertEqual(record.read_text(), 'preserve')
