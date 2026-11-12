"""Bounded SSH upgrades and rollback on project-owned private installations; never root/live devices."""
import copy
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import shutil
import socket
import time
import uuid
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from runtime import CHECKS, ROOT
from tools import remote_upgrade, remote_upgrade_agent as agent


def row(name, data, mode=0o755):
    return {'role': name, 'size': len(data), 'sha256': hashlib.sha256(data).hexdigest(), 'mode': mode}


def private_installation(test):
    # Publication deliberately refuses group-writable ancestors. Fixture directories
    # must model a privileged installation even when the developer uses umask 0002.
    previous = os.umask(0o022)
    test.addCleanup(os.umask, previous)


class PublishTest(unittest.TestCase):
    def setUp(self):
        private_installation(self)
        self.directory = tempfile.TemporaryDirectory(prefix='remote-upgrade-', dir=CHECKS)
        self.addCleanup(self.directory.cleanup)
        self.prefix = Path(self.directory.name)
        self.context = patch.object(agent, 'PREFIX', self.prefix)
        self.context.start()
        self.addCleanup(self.context.stop)
        self.addCleanup(agent.support.__setitem__, 'PREFIX', agent.support['PREFIX'])
        self.data = [b'new-daemon', b'new-manual']
        self.layout = {'graphics': False, 'files': [row('bin/worminald', self.data[0]),
                                                  row('share/man/man1/worminal.1', self.data[1], 0o644)]}
        for item in self.layout['files']:
            path = self.prefix / item['role']
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b'old:' + item['role'].encode())
            item['previous'] = agent.digest(path)
        self.old = [(self.prefix / item['role']).stat().st_ino for item in self.layout['files']]

    def publish(self, data=None):
        agent.publish(self.layout, io.BytesIO(b''.join(self.data) if data is None else data))

    def test_atomic_upgrade_matches_hashes_and_leaves_no_transaction_files(self):
        self.publish()
        for item, data in zip(self.layout['files'], self.data):
            path = self.prefix / item['role']
            self.assertEqual(path.read_bytes(), data)
            self.assertEqual(path.stat().st_mode & 0o777, item['mode'])
        self.assertEqual(list(self.prefix.rglob('.worminal-upgrade-*')), [])

    def test_group_writable_installation_directory_is_refused(self):
        (self.prefix / 'bin').chmod(0o775)
        with self.assertRaisesRegex(ValueError, 'writable by another user'):
            self.publish()
        self.assertEqual([agent.digest(self.prefix / item['role']) for item in self.layout['files']],
                         [item['previous'] for item in self.layout['files']])

    def test_failed_second_exchange_restores_original_inodes(self):
        original = agent.exchange
        calls = []
        def fail(parent, left, right):
            calls.append(right)
            if len(calls) == 2:
                raise OSError('injected publication failure')
            original(parent, left, right)
        with patch.object(agent, 'exchange', side_effect=fail), self.assertRaises(OSError):
            self.publish()
        self.assertEqual([agent.digest(self.prefix / item['role']) for item in self.layout['files']],
                         [item['previous'] for item in self.layout['files']])
        self.assertEqual([(self.prefix / item['role']).stat().st_ino for item in self.layout['files']], self.old)
        self.assertEqual(list(self.prefix.rglob('.worminal-upgrade-*')), [])

    def test_bad_transfer_and_changed_install_leave_originals(self):
        for data in (b''.join(self.data)[:-1], b'x' * sum(map(len, self.data))):
            with self.assertRaises(ValueError):
                self.publish(data)
            self.assertEqual([(self.prefix / item['role']).stat().st_ino for item in self.layout['files']], self.old)
        (self.prefix / 'bin/worminald').write_bytes(b'concurrent-release')
        with self.assertRaises(ValueError):
            self.publish()
        self.assertEqual((self.prefix / 'bin/worminald').read_bytes(), b'concurrent-release')

    def test_missing_session_process_is_not_reported_as_preserved(self):
        with patch.object(agent, 'owners', return_value=[]):
            result = agent.activation([], {'999999999': '123'}, self.layout, timeout=0)
        self.assertFalse(result['active'])
        self.assertEqual(result['session_processes_missing'], [999999999])

    def test_symlink_and_layout_mismatch_are_refused(self):
        destination = self.prefix / 'bin/worminald'
        destination.unlink()
        outside = self.prefix / 'unrelated'
        outside.write_bytes(b'keep')
        destination.symlink_to(outside)
        with self.assertRaises(OSError):
            self.publish()
        self.assertEqual(outside.read_bytes(), b'keep')
        layout = copy.deepcopy(self.layout)
        layout['files'][0]['role'] = '../outside'
        with self.assertRaises(ValueError):
            agent.validate(layout)

    def test_pending_legacy_owner_is_reported_without_signalling(self):
        inode = (self.prefix/'bin/worminald').stat().st_ino
        before = [{'pid': 123, 'role': 'worminald', 'inode': inode - 1, 'modules': {}}]
        with patch.object(agent, 'owners', return_value=before):
            result = agent.activation(before, {}, self.layout, timeout=0)
        self.assertFalse(result['active'])
        self.assertEqual(result['pending_owner_pids'], [123])


class CommandTest(unittest.TestCase):
    def setUp(self):
        private_installation(self)
        self.directory = tempfile.TemporaryDirectory(prefix='upgrade-command-', dir=CHECKS)
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.prefix = self.root/'source'
        (self.prefix/'bin').mkdir(parents=True)
        (self.prefix/'share/man/man1').mkdir(parents=True)
        (self.prefix/'bin/worminald').write_bytes(b'fixture-daemon')
        (self.prefix/'share/man/man1/worminal.1').write_bytes(b'fixture-manual')
        self.ssh = self.root/'ssh'
        self.ssh.write_text('#!/usr/bin/python3\nimport json,os,sys\n'
            'assert sys.argv[1:5]==["-T","-oBatchMode=yes","-oConnectTimeout=10","fixture-host"]\n'
            'mode=os.environ.get("UPGRADE_SCENARIO", "success")\n'
            'machine="wrong" if mode=="architecture" else '+repr(platform.machine())+'\n'
            'print(json.dumps({"ready":True,"system":"Linux","machine":machine}),flush=True)\n'
            'payload=sys.stdin.buffer.read()\n'
            'open(os.environ["UPGRADE_PAYLOAD"],"wb").write(payload)\n'
            'if mode=="lost":sys.exit(1)\n'
            'print(json.dumps({"ok":True,"installed":True,"active":mode!="pending"}))\n')
        self.ssh.chmod(0o700)
        self.env = patch.dict(os.environ, {'PATH': str(self.root)+os.pathsep+os.environ['PATH'],
                                          'UPGRADE_PAYLOAD': str(self.root/'received')})
        self.env.start()
        self.addCleanup(self.env.stop)

    def test_installed_source_snapshot_not_repo_build_is_transferred(self):
        result = remote_upgrade.upgrade('fixture-host', self.prefix)
        self.assertTrue(result['active'])
        self.assertEqual((self.root/'received').read_bytes(), b'fixture-daemonfixture-manual')

    def test_architecture_mismatch_sends_no_artifacts(self):
        with patch.dict(os.environ, {'UPGRADE_SCENARIO': 'architecture'}), self.assertRaises(ValueError):
            remote_upgrade.upgrade('fixture-host', self.prefix)
        self.assertFalse((self.root/'received').exists() and (self.root/'received').stat().st_size)

    def test_pending_and_lost_receipt_are_distinguished(self):
        with patch.dict(os.environ, {'UPGRADE_SCENARIO': 'pending'}):
            self.assertFalse(remote_upgrade.upgrade('fixture-host', self.prefix)['active'])
        with patch.dict(os.environ, {'UPGRADE_SCENARIO': 'lost'}), self.assertRaisesRegex(RuntimeError, 'uncertain'):
            remote_upgrade.upgrade('fixture-host', self.prefix)

    def test_host_injection_and_incomplete_source_are_refused(self):
        for host in ('-oProxyCommand=bad', 'host;echo bad', 'user@host\ncommand'):
            with self.assertRaises(ValueError):
                remote_upgrade.upgrade(host, self.prefix)
        (self.prefix/'bin/worminal').write_bytes(b'partial-desktop-install')
        with self.assertRaises(FileNotFoundError):
            remote_upgrade.snapshot(self.prefix)


class EmbeddedAgentTest(unittest.TestCase):
    def setUp(self):
        CommandTest.setUp(self)
        self.target = self.root/'target'
        (self.target/'bin').mkdir(parents=True)
        (self.target/'share/man/man1').mkdir(parents=True)
        (self.target/'bin/worminald').write_bytes(b'old-daemon')
        (self.target/'share/man/man1/worminal.1').write_bytes(b'old-manual')
        self.daemon = self.prefix/'bin/worminald'
        self.daemon.write_bytes(b'#!/bin/sh\necho Worp\n')
        self.ssh.write_text('#!/usr/bin/python3\nimport runpy\n'
                            f'runpy.run_path({str(ROOT/"tests/remote_upgrade_ssh.py")!r},run_name="__main__")\n')
        sudo = self.root/'sudo'
        sudo.write_text('#!/usr/bin/python3\nimport os,sys\n'
                        'if sys.argv[1:]==["-n","true"]:sys.exit(0)\n'
                        'assert sys.argv[1]=="-n"\nos.execv(sys.argv[2],sys.argv[2:])\n')
        sudo.chmod(0o700)
        os.environ.update(UPGRADE_TARGET_PREFIX=str(self.target), XDG_CACHE_HOME=str(self.root/'cache'))

    def test_embedded_agent_stages_checks_and_publishes_then_noops(self):
        result = remote_upgrade.upgrade('fixture-host', self.prefix)
        self.assertTrue(result['installed'] and result['active'] and result['changed'])
        self.assertFalse(result['started'])
        self.assertEqual((self.target/'bin/worminald').read_bytes(), self.daemon.read_bytes())
        for record in result['files']:
            self.assertEqual(agent.digest(self.target/record['path']), record['sha256'])
        self.assertFalse(remote_upgrade.upgrade('fixture-host', self.prefix)['changed'])

    def test_native_daemon_upgrade_preserves_live_shell_and_pty(self):
        from test_handoff import HandoffFixture
        from workspace_protocol import service_pid, stop_workspace
        from tools.daemon_upgrade import terminal_descriptors
        native = Path(os.environ.get('WORMINAL_TEST_PREVIEW_DAEMON', ROOT/'build/cargo/release/worminal-daemon'))
        if not native.is_file(): self.skipTest('Build the Rust daemon for native live-upgrade verification')
        shutil.copy2(native, self.daemon)
        target_daemon = self.target/'bin/worminald'
        shutil.copy2(native, target_daemon)
        with target_daemon.open('ab') as output: output.write(b'old-fixture-generation')
        env = {**os.environ, 'WORMINAL_SHARED_SOCKET_SCOPE': 'remote-upgrade-'+uuid.uuid4().hex,
               'XDG_STATE_HOME': str(self.root/'state')}
        with patch.dict(os.environ, env):
            process = subprocess.Popen([str(target_daemon), '--serve'], env=env,
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            peer = socket.socket(socket.AF_UNIX)
            peer.settimeout(5)
            try:
                deadline = time.monotonic()+5
                while True:
                    try:
                        peer.connect(f"\0worminal-workspace-v2-{os.getuid()}-{env['WORMINAL_SHARED_SOCKET_SCOPE']}")
                        break
                    except ConnectionRefusedError:
                        if time.monotonic()>deadline: raise
                        time.sleep(.02)
                _, shell = HandoffFixture.launch(self, peer, 2, 'remote upgrade retained')
                started = Path(f'/proc/{shell}/stat').read_text().rsplit(')', 1)[1].split()[19]
                ptys = terminal_descriptors(process.pid)
                peer.close()
                result = remote_upgrade.upgrade('fixture-host', self.prefix)
                self.assertTrue(result['active'])
                self.assertGreaterEqual(result['preserved_session_processes'], 1)
                self.assertEqual(Path(f'/proc/{shell}/stat').read_text().rsplit(')', 1)[1].split()[19], started)
                with socket.socket(socket.AF_UNIX) as current:
                    current.settimeout(5)
                    current.connect(f"\0worminal-workspace-v2-{os.getuid()}-{env['WORMINAL_SHARED_SOCKET_SCOPE']}")
                    replacement = service_pid(current)
                self.assertNotEqual(replacement, process.pid)
                self.assertEqual(terminal_descriptors(replacement), ptys)
            finally:
                peer.close()
                stop_workspace(env)
                process.wait(timeout=5)

    def test_incompatible_runtime_leaves_existing_target_untouched(self):
        self.daemon.write_bytes(b'#!/bin/sh\nexit 9\n')
        with self.assertRaisesRegex(RuntimeError, 'Staged daemon'):
            remote_upgrade.upgrade('fixture-host', self.prefix)
        self.assertEqual((self.target/'bin/worminald').read_bytes(), b'old-daemon')



if __name__ == '__main__':
    unittest.main()
