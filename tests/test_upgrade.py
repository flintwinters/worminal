"""Automatic daemon replacement retains the endpoint, identities, PTYs, and recovery recipes."""
import os
import signal
import select
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
from workspace_protocol import catalog_records, number, send, string, wait_kind, service_pid, rust_daemon


class UpgradeTest(HandoffFixture, unittest.TestCase):
    def setUp(self):
        super().setUp()
        (ROOT / '.checks').mkdir(exist_ok=True)
        self.directory = tempfile.TemporaryDirectory(prefix='upgrade-', dir=ROOT / '.checks')
        self.addCleanup(self.directory.cleanup)
        self.binary = Path(self.directory.name) / 'worminald'
        candidate = os.environ.get("WORMINAL_TEST_UPGRADE_CANDIDATE", BINARIES[2])
        self.rust = rust_daemon(candidate)
        shutil.copy2(candidate, self.binary)

    def track_receivers(self, constructor):
        def tracked(*args, **kwargs):
            pid, token = constructor(*args, **kwargs)
            self.addCleanup(self.retire_receiver, pid, os.pidfd_open(pid), args[1])
            return pid, token
        return tracked

    def retire_receiver(self, pid, handle, scope):
        try:
            env = {**os.environ, 'WORMINAL_SHARED_SOCKET_SCOPE': scope}
            subprocess.run([str(BINARIES[2]), '--stop'], env=env, timeout=3,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            deadline = time.monotonic() + 2
            while True:
                try:
                    if os.waitpid(pid, os.WNOHANG)[0]: break
                except ChildProcessError:
                    break  # The successful bootstrap already reaped its staging receiver.
                if time.monotonic() >= deadline:
                    try: signal.pidfd_send_signal(handle, signal.SIGKILL)
                    except ProcessLookupError: pass
                    os.waitpid(pid, 0)
                    break
                time.sleep(.01)
        finally:
            os.close(handle)

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

    def changed(self, env, previous):
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            try:
                peer = self.connect(env)
                pid = service_pid(peer)
            except (ConnectionRefusedError, ConnectionResetError):
                time.sleep(.02)
                continue
            except AssertionError as error:
                if str(error) != 'workspace closed its connection': raise
                time.sleep(.02)
                continue
            if pid != previous:
                self.assertEqual(os.getsid(pid), pid)
                self.assertTrue(Path(f'/proc/{pid}/exe').samefile(self.binary))
                return peer
            time.sleep(.02)
        self.fail('replacement did not take ownership')

    def test_upgrade_and_reverse_rollback_preserve_shell_identity_and_order(self):
        peer, env, parent = self.start(2, self.binary)
        removed, _ = self.launch(peer, 2, 'removed')
        tab, shell_pid = self.launch(peer, 2, 'retained')
        send(peer, 5, removed, version=2)
        send(peer, 17, payload=string('https://example.invalid/rollback'), version=2)
        wait_kind(peer, 20, version=2)  # Drain the preceding close catalog before observing browser creation.
        expected = self.catalog(peer)
        self.assertEqual([row[0] for row in expected], [tab, tab + 1])
        terminal = os.readlink(f'/proc/{shell_pid}/fd/0')
        previous = parent.pid
        other = os.environ.get('WORMINAL_TEST_UPGRADE_PEER', BINARIES[2])
        for index, executable in enumerate((other, BINARIES[2])):
            self.replace(executable)
            peer = self.changed(env, previous)
            previous = service_pid(peer)
            self.assertEqual(self.catalog(peer), expected)
            self.assertEqual(os.readlink(f'/proc/{shell_pid}/fd/0'), terminal)
            self.assertEqual(os.getsid(shell_pid), shell_pid)
            send(peer, 4, tab, number(40) + number(8), version=2)
            marker = f'unchanged-shell-{index}'
            send(peer, 3, tab, marker.encode(), version=2)
            self.assert_screen(peer, 2, tab, marker)
        self.assertEqual(parent.wait(timeout=3), 0)

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

    def test_receiver_setup_failure_retains_donor_before_commit(self):
        library = Path(self.directory.name) / 'setup-fault.so'
        marker = Path(self.directory.name) / 'enable-fault'
        marker.touch()
        subprocess.run(['cc', '-shared', '-fPIC', '-Wall', '-Wextra', '-o', str(library),
                        str(ROOT / 'tests/upgrade_fault.c'), '-ldl'], check=True)
        env = {'LD_PRELOAD': str(library), 'WORMINAL_TEST_UPGRADE_FAULT_FILE': str(marker),
               'WORMINAL_TEST_UPGRADE_FAULT_OPERATION':
                   'signals' if self.rust else 'transport'}
        peer, env, parent = self.start(2, self.binary, env)
        tab, shell_pid = self.launch(peer, 2, 'setup-retained')
        self.replace()
        deadline = time.monotonic() + 8
        output = b''
        while b'Daemon upgrade rejected; live tabs retained' not in output and time.monotonic() < deadline:
            if select.select([parent.stderr], [], [], max(0, deadline - time.monotonic()))[0]:
                chunk = os.read(parent.stderr.fileno(), 4096)
                if not chunk: break
                output += chunk
        self.assertIn(b'Daemon upgrade rejected; live tabs retained', output)
        self.assertIsNone(parent.poll())
        self.assertTrue(Path(f'/proc/{shell_pid}/fd/0').exists())
        send(peer, 3, tab, b'setup-rejected', version=2)
        self.assert_screen(peer, 2, tab, 'setup-rejected')
        marker.unlink()
        self.replace()
        replacement = self.completed(parent, env)
        self.assertEqual([row[0] for row in self.catalog(replacement)], [tab])
        send(replacement, 4, tab, number(40) + number(8), version=2)
        send(replacement, 3, tab, b' after-retry', version=2)
        self.assert_screen(replacement, 2, tab, 'after-retry')

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
        from tools import daemon_upgrade
        baseline = os.environ.get('WORMINAL_TEST_HANDOFF_BASELINE')
        old = Path(self.directory.name) / 'previous'
        shutil.copy2(baseline or BINARIES[2], old)
        peer, env, parent = self.start(2, old)
        removed, _ = self.launch(peer, 2, 'removed')
        tab, pid = self.launch(peer, 2, 'retained')
        send(peer, 5, removed, version=2)
        expected = self.catalog(peer)
        original_ready = daemon_upgrade.wait_ready

        def verified_ready(scope, expected_pid, rows):
            original_ready(scope, expected_pid, rows)
            if scope != env['WORMINAL_SHARED_SOCKET_SCOPE']:
                deadline = time.monotonic() + 1
                while True:
                    state = Path(f'/proc/{parent.pid}/stat').read_text().rsplit(') ', 1)[1][0]
                    if state in ('T', 't'): break
                    if time.monotonic() >= deadline:
                        self.fail('legacy donor was not paused before acknowledging ownership')
                    time.sleep(.01)

        with patch.dict(os.environ, env), \
                patch.object(daemon_upgrade, 'receiver', self.track_receivers(daemon_upgrade.receiver)), \
                patch.object(daemon_upgrade, 'wait_ready', verified_ready):
            daemon_upgrade.upgrade(self.binary)
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

    def test_invalid_legacy_mapping_retains_donor_and_its_shell(self):
        sys_path = str(ROOT)
        if sys_path not in sys.path: sys.path.insert(0, sys_path)
        from tools import daemon_upgrade
        peer, env, parent = self.start(2, self.binary)
        tab, shell_pid = self.launch(peer, 2, 'mapping-retained')
        receiver = self.track_receivers(daemon_upgrade.receiver)
        for index, ids in enumerate(([], [0], [tab, tab])):
            scope = self.token + f'-invalid-{index}'
            child, token = receiver(self.binary, scope, ids, env, parent.pid)
            result = subprocess.run([str(BINARIES[2]), '--handoff', token], env=env,
                                    capture_output=True, timeout=10)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(os.waitpid(child, 0)[1], 256)
            self.assertTrue(Path(f'/proc/{shell_pid}/fd/0').exists())
            marker = f'mapping-failure-{index}'
            send(peer, 3, tab, b'\r\n' + marker.encode(), version=2)
            self.assert_screen(peer, 2, tab, marker)

    def test_stopping_receiver_rejects_before_ready_and_preserves_donor(self):
        sys_path = str(ROOT)
        if sys_path not in sys.path: sys.path.insert(0, sys_path)
        from tools import daemon_upgrade
        peer, env, parent = self.start(2, self.binary)
        tab, shell_pid = self.launch(peer, 2, 'cancellation-retained')
        receiver = self.track_receivers(daemon_upgrade.receiver)
        child, token = receiver(self.binary, self.token + '-cancel', [tab], env)
        deadline = time.monotonic() + 3
        while True:
            status = Path(f'/proc/{child}/status').read_text().splitlines()
            caught = next(line.split()[1] for line in status if line.startswith('SigCgt:'))
            registered = int(caught, 16) & (1 << (signal.SIGTERM - 1))
            if self.rust:
                for descriptor in Path(f'/proc/{child}/fdinfo').iterdir():
                    try: info = descriptor.read_text()
                    except FileNotFoundError: continue
                    registered |= any(line.startswith('sigmask:') and
                                      int(line.split()[1], 16) & (1 << (signal.SIGTERM - 1))
                                      for line in info.splitlines())
            if registered: break
            if time.monotonic() >= deadline: self.fail('receiver did not register its stop signals')
            time.sleep(.01)
        os.kill(child, signal.SIGTERM)
        result = subprocess.run([str(BINARIES[2]), '--handoff', token], env=env,
                                capture_output=True, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue(Path(f'/proc/{shell_pid}/fd/0').exists())
        send(peer, 3, tab, b'cancelled-receiver', version=2)
        self.assert_screen(peer, 2, tab, 'cancelled-receiver')
