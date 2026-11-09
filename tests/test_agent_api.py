"""Native agent commands against a private workspace, never a desktop session."""
import json
import os
from pathlib import Path
import tempfile
import subprocess
import time
import unittest

import test_preview_workspace as workspace
from workspace_protocol import number, send, wait_kind


class AgentApiTest(unittest.TestCase):
    def setUp(self):
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

    def test_worp_ssh_transport_uses_stdio_and_preserves_literal_input(self):
        tab = self.fixture.launch('stty raw -echo; printf READY; exec cat')
        with tempfile.TemporaryDirectory(prefix='worp-ssh-', dir=workspace.CHECKS) as directory:
            shim = Path(directory) / 'ssh'
            shim.write_text('#!/usr/bin/python3\nimport os,sys\n'
                            'assert sys.argv[-3:] == ["worminald", "api", "--stdio"]\n'
                            'assert "fixture-host" in sys.argv\n'
                            f'os.execv({str(workspace.DAEMON)!r}, '
                            f'[{str(workspace.DAEMON)!r}, "api", "--stdio"])\n')
            shim.chmod(0o700)
            previous = self.fixture.env.get('PATH', '')
            self.fixture.env['PATH'] = directory + os.pathsep + previous
            try:
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
            finally:
                self.fixture.env['PATH'] = previous
