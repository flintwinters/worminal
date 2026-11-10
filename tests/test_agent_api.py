"""Native agent commands against a private workspace, never a desktop session."""
from contextlib import contextmanager
import json
import os
from pathlib import Path
import tempfile
import subprocess
import time
import unittest
from unittest.mock import patch

import test_preview_workspace as workspace
from workspace_protocol import number, send, wait_kind


class AgentApiTest(unittest.TestCase):
    def setUp(self):
        state = tempfile.TemporaryDirectory(prefix='agent-state-', dir=workspace.CHECKS)
        self.addCleanup(state.cleanup)
        environment = patch.dict(os.environ, {'XDG_STATE_HOME': state.name})
        environment.start()
        self.addCleanup(environment.stop)
        self.fixture = workspace.PreviewWorkspaceTest(methodName='runTest')
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)

    def api(self, *args, success=True):
        result = subprocess.run([str(workspace.DAEMON), 'api', *map(str, args)],
                                env=self.fixture.env, capture_output=True, text=True, timeout=5)
        self.assertEqual(result.returncode, 0 if success else 1, result.stderr + result.stdout)
        value = json.loads(result.stdout)
        self.assertEqual(value['ok'], success)
        return value

    def test_read_and_input_preserve_live_view_and_geometry(self):
        tab = self.fixture.launch('stty raw -echo; printf READY; exec cat')
        record = self.api('list')['tabs'][0]
        self.assertEqual(record['id'], tab)
        self.assertEqual(record['kind'], 'terminal')
        self.assertGreater(record['shell_pid'], 0)
        before = self.api('read', tab)
        self.assertEqual((before['columns'], before['rows']), (40, 4))
        self.assertIn('READY', '\n'.join(before['lines']))
        self.assertEqual(self.api('type', tab, 'hello 世界')['queued_bytes'], len('hello 世界'.encode()))
        self.api('key', tab, 'Enter')
        # The original graphical protocol connection still receives frames: one-shot
        # agent clients must not steal its controller and then detach it on exit.
        wait_kind(self.fixture.peer, 10, tab, version=2)
        deadline = time.monotonic() + 2
        while True:
            after = self.api('read', tab)
            if 'hello 世界' in '\n'.join(after['lines']):
                break
            self.assertLess(time.monotonic(), deadline, after)
            time.sleep(.01)
        self.assertEqual((after['columns'], after['rows']), (40, 4))
        # A protocol resize from the original controller must remain authorized.
        send(self.fixture.peer, 23, tab, number(42) + number(5), version=2)
        wait_kind(self.fixture.peer, 10, tab, version=2)
        resized = self.api('read', tab)
        self.assertEqual((resized['columns'], resized['rows']), (42, 5))

    def test_open_terminal_and_close_preserve_other_tabs_and_literal_argv(self):
        existing=self.fixture.launch('stty raw -echo; printf READY; exec cat')
        with tempfile.TemporaryDirectory(prefix='api-cwd-',dir=workspace.CHECKS) as directory:
            value=self.api('open','--cwd',directory,'--title','agent terminal','--columns','61','--rows','8',
                           '--','/bin/sh','-c','printf "%s\\n" "$1"; exec cat','literal argv','literal ; $HOME')
            tab=value['id'];pid=value['tab']['shell_pid']
            self.assertTrue(value['created']);self.assertGreater(pid,0)
            self.assertEqual(value['tab']['directory'],directory)
            self.assertEqual(value['tab']['title'],'agent terminal')
            deadline=time.monotonic()+3
            while 'literal ; $HOME' not in '\n'.join(self.api('read',tab)['lines']):
                self.assertLess(time.monotonic(),deadline)
                time.sleep(.01)
            self.assertEqual((self.api('read',tab)['columns'],self.api('read',tab)['rows']),(61,8))
            self.assertEqual([row['id'] for row in self.api('list')['tabs']],[existing,tab])
            self.assertTrue(self.api('close',tab)['closed'])
            self.assertEqual([row['id'] for row in self.api('list')['tabs']],[existing])
            deadline=time.monotonic()+3
            while Path(f'/proc/{pid}').exists():
                self.assertLess(time.monotonic(),deadline,'Closing did not terminate the owned process')
                time.sleep(.01)
            self.api('close',tab,success=False)
            self.assertTrue(self.api('read',existing)['ok'])

    def test_open_browser_and_lifecycle_over_ssh(self):
        with self.ssh_transport():
            value=self.api('--master','fixture-host','open','--browser','https://example.test/path')
            self.assertEqual(value['tab']['kind'],'browser')
            self.assertEqual(value['tab']['url'],'https://example.test/path')
            self.assertEqual(value['tab']['shell_pid'],0)
            self.api('--master','fixture-host','close',value['id'])
        self.assertEqual(self.api('list')['tabs'],[])

    def test_default_shell_and_fast_command_have_explicit_creation_receipts(self):
        default=self.api('open')
        self.assertEqual(default['tab']['kind'],'terminal')
        self.assertGreater(default['tab']['shell_pid'],0)
        self.api('close',default['id'])
        fast=self.api('open','--','/bin/true')
        self.assertTrue(fast['created'])
        self.assertTrue(self.api('list')['ok'])

    def test_invalid_open_has_no_orphan_tab_and_does_not_stop_service(self):
        for args in [('open','--cwd','relative'),('open','--rows','0'),
                     ('open','--','/no/such/worp-program'),('open','--cwd','/no/such/worp-directory'),
                     ('open','--browser','relative.html')]:
            self.api(*args,success=False)
            self.assertEqual(self.api('list')['tabs'],[])
        opened=self.api('open','--','/bin/cat')
        self.api('close',opened['id'])

    def test_explicit_errors_leave_workspace_unchanged(self):
        self.api('read', 999, success=False)
        self.api('key', 1, 'Unknown', success=False)
        self.api('type', 1, 'oops\n', success=False)
        self.api('read', 0, success=False)
        self.assertEqual(self.api('list')['tabs'], [])
        send(self.fixture.peer, 17, payload=workspace.string('https://example.invalid'), version=2)
        tab, _ = wait_kind(self.fixture.peer, 20, version=2)
        self.api('read', tab, success=False)
        self.api('type', tab, 'text', success=False)
        self.assertEqual(len(self.api('list')['tabs']), 1)

    def test_help_and_missing_service_have_no_startup_side_effects(self):
        environment = {**self.fixture.env, 'WORMINAL_SHARED_SOCKET_SCOPE': self.fixture.scope + '-absent'}
        for args in (['--help'], ['api'], ['api', 'help']):
            result = subprocess.run([str(workspace.DAEMON), *args], env=environment,
                                    capture_output=True, text=True, timeout=5)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn('api', result.stdout)
        result = subprocess.run([str(workspace.DAEMON), 'api', 'list'], env=environment,
                                capture_output=True, text=True, timeout=5)
        self.assertEqual(result.returncode, 1)
        self.assertFalse(json.loads(result.stdout)['ok'])

    @contextmanager
    def session(self):
        process = subprocess.Popen([str(workspace.DAEMON), 'api', '--session'],
                                   env=self.fixture.env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, bufsize=0)
        class Pipe:
            def sendall(self, data):
                process.stdin.write(data)
                process.stdin.flush()
            def recv(self, count):
                import select
                if not select.select([process.stdout], [], [], 5)[0]:
                    raise AssertionError('Worp session response timed out')
                return os.read(process.stdout.fileno(), count)
        try:
            yield Pipe()
        finally:
            process.terminate()
            process.wait(timeout=5)
            for stream in (process.stdin, process.stdout, process.stderr):
                stream.close()

    def test_persistent_worp_session_uses_same_tab_input_resize_and_lifecycle(self):
        existing = self.fixture.launch('stty raw -echo; printf READY; exec cat')
        with self.session() as session:
            send(session, 19, version=2)  # Subscribe to the existing catalog.
            wait_kind(session, 7, version=2)
            send(session, 4, existing, number(43) + number(6), version=2)
            wait_kind(session, 10, existing, version=2)
            self.assertEqual(self.api('read', existing)['columns'], 43)
            # Unary input preserves this subscribed controller's resize authority.
            self.api('type', existing, 'shared contract')
            wait_kind(session, 10, existing, version=2)
            send(session, 23, existing, number(44) + number(7), version=2)
            wait_kind(session, 10, existing, version=2)
            self.assertEqual(self.api('read', existing)['columns'], 44)
            send(session, 17, payload=workspace.string('https://first.test/'), version=2)
            browser, _ = wait_kind(session, 20, version=2)
            self.assertEqual(self.api('list')['tabs'][-1]['id'], browser)
            send(session, 18, browser, workspace.string('https://next.test/'), version=2)
            wait_kind(session, 7, version=2)
            deadline = time.monotonic() + 2
            while self.api('list')['tabs'][-1]['url'] != 'https://next.test/':
                self.assertLess(time.monotonic(), deadline)
            send(session, 5, browser, version=2)
            deadline = time.monotonic() + 2
            while len(self.api('list')['tabs']) != 1:
                self.assertLess(time.monotonic(), deadline)
            self.assertTrue(self.api('read', existing)['ok'])

    @contextmanager
    def ssh_transport(self):
        with tempfile.TemporaryDirectory(prefix='worp-ssh-', dir=workspace.CHECKS) as directory:
            log = Path(directory) / 'calls'
            shim = Path(directory) / 'ssh'
            shim.write_text('#!/usr/bin/python3\nimport os,sys\n'
                            'assert sys.argv[-3:] == ["worminald", "api", "--stdio"]\n'
                            f'with open({str(log)!r}, "a") as out: out.write(sys.argv[-4]+"\\n")\n'
                            'if "offline-host" in sys.argv: sys.exit(255)\n'
                            'assert "fixture-host" in sys.argv\n'
                            f'os.execv({str(workspace.DAEMON)!r}, '
                            f'[{str(workspace.DAEMON)!r}, "api", "--stdio"])\n')
            shim.chmod(0o700)
            previous = self.fixture.env.get('PATH', '')
            self.fixture.env['PATH'] = directory + os.pathsep + previous
            try:
                yield log
            finally:
                self.fixture.env['PATH'] = previous

    def test_worp_ssh_transport_uses_stdio_and_preserves_literal_input(self):
        tab = self.fixture.launch('stty raw -echo; printf READY; exec cat')
        with self.ssh_transport():
            value = self.api('--master', 'fixture-host', 'list')
            self.assertEqual(value['tabs'][0]['id'], tab)
            # Text travels in the Worp request, never in SSH's shell command.
            text = 'literal $HOME; "quoted"'
            self.api('--master', 'fixture-host', 'type', tab, text)
            self.api('--master', 'fixture-host', 'key', tab, 'Enter')
            value = self.api('--master', 'fixture-host', 'read', tab)
            self.assertIn(text, '\n'.join(value['lines']))
            # Remote forwarding also must not create a workspace during discovery.
            scope = self.fixture.env['WORMINAL_SHARED_SOCKET_SCOPE']
            self.fixture.env['WORMINAL_SHARED_SOCKET_SCOPE'] = scope + '-absent'
            try:
                self.api('--master', 'fixture-host', 'list', success=False)
            finally:
                self.fixture.env['WORMINAL_SHARED_SOCKET_SCOPE'] = scope

    def test_explicit_registry_on_a_shared_authority_and_caller_availability(self):
        with self.ssh_transport() as log:
            def authority(*args, **kwargs):
                return self.api('--master', 'fixture-host', *args, **kwargs)
            self.assertEqual(authority('devices', '--no-check')['devices'], [])
            enrolled = authority('enroll', 'bench', 'fixture-host')
            self.assertTrue(enrolled['changed'])
            self.assertFalse(authority('enroll', 'bench', 'fixture-host')['changed'])
            authority('enroll', 'bench', 'another-host', success=False)
            authority('enroll', 'alias', 'fixture-host', success=False)
            authority('enroll', 'offline', 'offline-host')
            calls = log.read_text().splitlines()
            self.assertNotIn('offline-host', calls, 'enrollment must never contact the endpoint')
            value = self.api('devices', '--no-check')
            self.assertEqual(log.read_text().splitlines(), calls, 'membership inspection must not probe')
            self.assertTrue(all(row['availability'] == 'unchecked' for row in value['devices']))
            value = authority('devices')
            rows = {row['name']: row for row in value['devices']}
            self.assertEqual(rows['bench']['availability'], 'reachable')
            self.assertEqual(rows['offline']['availability'], 'unavailable')
            self.assertIsInstance(rows['offline']['checked_at_unix_ms'], int)
            self.assertEqual(value['availability_vantage'], 'caller')
            self.assertEqual(value['registry_authority'], 'fixture-host')
            self.assertEqual(len(self.api('devices', '--no-check')['devices']), 2)
            self.assertTrue(authority('forget', 'offline')['removed'])
            self.assertFalse(authority('forget', 'offline')['removed'])
            self.api('enroll', '../escape', 'host', success=False)
            self.api('enroll', 'node', 'ssh://host', success=False)

    def test_registry_survives_daemon_restart_without_auto_enrollment(self):
        self.assertEqual(self.api('devices', '--no-check')['devices'], [])
        self.api('enroll', 'bench', 'fixture-host')
        self.fixture.peer.close()
        self.fixture.stop()
        self.fixture.daemon = subprocess.Popen([str(workspace.DAEMON), '--serve'], env=self.fixture.env,
                                               stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        self.fixture.peer = self.fixture.connect()
        self.addCleanup(self.fixture.peer.close)
        rows = self.api('devices', '--no-check')['devices']
        self.assertEqual([(row['name'], row['endpoint']) for row in rows], [('bench', 'fixture-host')])
