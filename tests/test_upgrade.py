"""Automatic daemon replacement retains the endpoint, identities, PTYs, and recovery recipes."""
import os
import signal
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import struct
import tempfile
import time
import unittest
from unittest.mock import patch

from test_handoff import HandoffFixture, ROOT, BINARIES
from workspace_protocol import catalog_records, number, send, string, wait_kind, service_pid


class UpgradeTest(HandoffFixture, unittest.TestCase):
    def setUp(self):
        super().setUp()
        (ROOT / '.checks').mkdir(exist_ok=True)
        self.directory = tempfile.TemporaryDirectory(prefix='upgrade-', dir=ROOT / '.checks')
        self.addCleanup(self.directory.cleanup)
        self.binary = Path(self.directory.name) / 'worminald'
        shutil.copy2(os.environ.get("WORMINAL_TEST_UPGRADE_CANDIDATE", BINARIES[2]), self.binary)

    def connect(self, env):
        peer = socket.socket(socket.AF_UNIX)
        peer.settimeout(8)
        peer.connect(f"\0worminal-workspace-v2-{os.getuid()}-{env['WORMINAL_SHARED_SOCKET_SCOPE']}")
        self.peers.append(peer)
        return peer

    def catalog(self, peer):
        send(peer, 19, version=2)
        return catalog_records(wait_kind(peer, 7, version=2)[1])

    def replace(self, executable=None):
        pending = self.binary.with_suffix('.pending')
        shutil.copy2(executable or BINARIES[2], pending)
        pending.replace(self.binary)

    def completed(self, process, env):
        self.assertEqual(process.wait(timeout=10), 0)
        peer = self.connect(env)
        pid = service_pid(peer)
        self.assertNotEqual(pid, process.pid)
        self.assertEqual(os.getsid(pid), pid, "replacement must have an independent daemon session")
        self.assertTrue(Path(f'/proc/{pid}/exe').samefile(self.binary))
        return peer

    def test_replacement_preserves_gapped_ids_browser_and_live_shells(self):
        peer, env, parent = self.start(2, self.binary)
        first, first_pid = self.launch(peer, 2, 'first')
        removed, _ = self.launch(peer, 2, 'removed')
        third, third_pid = self.launch(peer, 2, 'third')
        send(peer, 5, removed, version=2)
        send(peer, 3, first, b'before-upgrade', version=2)
        self.assert_screen(peer, 2, first, 'before-upgrade')
        send(peer, 17, payload=string('https://example.invalid/retained'), version=2)
        expected = self.catalog(peer)
        self.assertEqual([row[0] for row in expected], [first, third, third + 1])
        self.replace()
        replacement = self.completed(parent, env)
        self.assertEqual(self.catalog(replacement), expected)
        for pid in (first_pid, third_pid):
            self.assertTrue(Path(f'/proc/{pid}').exists(), 'upgrade replaced a live shell')
        send(replacement, 4, first, number(40) + number(8), version=2)
        self.assert_screen(replacement, 2, first, 'before-upgrade')
        send(replacement, 3, first, b' after-upgrade', version=2)
        self.assert_screen(replacement, 2, first, 'after-upgrade')
        state_root = Path(env.get('XDG_STATE_HOME', Path.home() / '.local/state'))
        state = state_root / 'worminal' / f"tabs-v2-{env['WORMINAL_SHARED_SOCKET_SCOPE'].encode().hex()}"
        deadline = time.monotonic() + 3
        while not state.exists() or struct.unpack('!I', state.read_bytes()[4:8])[0] != 3:
            if time.monotonic() >= deadline:
                self.fail('replacement did not checkpoint the imported workspace')
            time.sleep(.01)

    def test_empty_workspace_replaces_without_resurrecting_tabs(self):
        _, env, parent = self.start(2, self.binary)
        self.replace()
        peer = self.completed(parent, env)
        self.assertEqual(self.catalog(peer), [])
        tab, pid = self.launch(peer, 2, 'new')
        self.assertEqual(tab, 1)
        self.assertTrue(Path(f'/proc/{pid}').exists())

    def test_rejected_replacement_retains_shell_and_retries_next_install(self):
        peer, env, parent = self.start(2, self.binary)
        tab, pid = self.launch(peer, 2, 'retained')
        invalid = Path(self.directory.name) / 'unsupported'
        invalid.write_text('#!/usr/bin/python3\nimport signal\n'
                           'signal.signal(signal.SIGTERM, signal.SIG_IGN)\n'
                           'while True: signal.pause()\n')
        invalid.chmod(0o700)
        self.replace(invalid)
        # The old donor times out before commit, retaining exclusive PTY ownership.
        time.sleep(5.3)
        self.assertIsNone(parent.poll())
        self.assertTrue(Path(f'/proc/{pid}').exists())
        send(peer, 3, tab, b'retained-after-rejection', version=2)
        self.assert_screen(peer, 2, tab, 'retained-after-rejection')
        self.replace()
        replacement = self.completed(parent, env)
        self.assertEqual([row[0] for row in self.catalog(replacement)], [tab])
        send(replacement, 4, tab, number(40) + number(8), version=2)
        self.assert_screen(replacement, 2, tab, 'retained-after-rejection')

    def test_bootstrap_survives_coordinator_process_group_cleanup(self):
        old = Path(self.directory.name) / 'previous'
        shutil.copy2(BINARIES[2], old)
        peer, env, parent = self.start(2, old)
        tab, shell_pid = self.launch(peer, 2, 'retained-context')
        send(peer, 3, tab, b'original-context', version=2)
        self.assert_screen(peer, 2, tab, 'original-context')
        coordinator = subprocess.Popen(
            [sys.executable, str(ROOT / 'manage.py'), 'upgrade-daemon', '--executable', str(self.binary)],
            cwd=ROOT, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
        _, errors = coordinator.communicate(timeout=15)
        self.assertEqual(coordinator.returncode, 0, errors.decode())
        replacement = self.connect(env)
        replacement_pid = service_pid(replacement)
        self.assertEqual([row[0] for row in self.catalog(replacement)], [tab])
        # Command runners clean up their process groups after the command exits.
        # A transferred daemon must have its own session before claiming PTYs.
        try:
            os.killpg(coordinator.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        time.sleep(.1)
        self.assertEqual(os.getsid(replacement_pid), replacement_pid,
                         'transferred daemon inherited the coordinator session')
        self.assertTrue(Path(f'/proc/{shell_pid}').exists(), 'coordinator cleanup killed the original shell')
        self.assertEqual(service_pid(replacement), replacement_pid)
        send(replacement, 4, tab, number(40) + number(8), version=2)
        self.assert_screen(replacement, 2, tab, 'original-context')

    def test_bootstrap_legacy_handoff_preserves_ids_and_enables_checkpoints(self):
        sys_path = str(ROOT)
        import sys
        if sys_path not in sys.path:
            sys.path.insert(0, sys_path)
        from tools.daemon_upgrade import upgrade
        baseline = os.environ.get('WORMINAL_TEST_HANDOFF_BASELINE')
        old = Path(self.directory.name) / 'previous'
        shutil.copy2(baseline or BINARIES[2], old)
        peer, env, parent = self.start(2, old)
        removed, _ = self.launch(peer, 2, 'removed')
        tab, pid = self.launch(peer, 2, 'retained')
        send(peer, 5, removed, version=2)
        expected = self.catalog(peer)
        with patch.dict(os.environ, env):
            replacement_pid = upgrade(self.binary)
        self.addCleanup(os.waitpid, replacement_pid, 0)
        self.assertEqual(parent.wait(timeout=3), -signal.SIGKILL)
        replacement = self.connect(env)
        self.assertEqual(self.catalog(replacement), expected)
        # The old view must see a disconnect, never an empty catalog that would
        # destroy its windows and browser DOM before the reconnect.
        from workspace_protocol import packet
        while True:
            try:
                kind, _, payload = packet(peer, version=2)
            except (AssertionError, ConnectionResetError):
                break
            if kind == 7:
                self.assertTrue(catalog_records(payload), 'bootstrap published an empty donor catalog')
        self.assertTrue(Path(f'/proc/{pid}').exists())
        send(replacement, 4, tab, number(40) + number(8), version=2)
        send(replacement, 3, tab, b'bootstrap-complete', version=2)
        self.assert_screen(replacement, 2, tab, 'bootstrap-complete')
