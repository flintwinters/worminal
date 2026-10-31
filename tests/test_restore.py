"""Durable tab recipes through real daemon and zsh restarts, on isolated workspaces."""
import os
from pathlib import Path
import shlex
import signal
import select
import socket
import struct
import subprocess
import tempfile
import time
import unittest
import uuid

from workspace_protocol import number, string, send, wait_kind, packet

ROOT = Path(__file__).resolve().parent.parent


class RestoreTest(unittest.TestCase):
    version = 2

    def setUp(self):
        (ROOT / '.checks').mkdir(exist_ok=True)
        self.temporary = tempfile.TemporaryDirectory(prefix='restore-', dir=ROOT / '.checks')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.scope = 'restore-' + uuid.uuid4().hex
        self.default = self.root / 'default'
        self.default.mkdir()
        self.shell = self.root / 'shell'
        self.shell.write_text('#!/bin/sh\nexec /bin/zsh -d\n')
        self.shell.chmod(0o700)
        (self.root / '.zshrc').write_text(
            '[[ -n ${WORMINAL_RESTORE-} ]] || cd -- "$RESTORE_DEFAULT"\n'
            'print -r -- "$PWD|${WORMINAL_RESTORE-unset}" >> "$RESTORE_LOG"\n'
            'unset WORMINAL_RESTORE\n')
        self.env = {**os.environ, 'XDG_STATE_HOME': str(self.root / 'state'),
                    'WORMINAL_SHARED_SOCKET_SCOPE': self.scope, 'SHELL': str(self.shell),
                    'ZDOTDIR': str(self.root), 'RESTORE_DEFAULT': str(self.default),
                    'RESTORE_LOG': str(self.root / 'startup')}
        self.process = None
        self.peers = []
        self.addCleanup(self.cleanup)
        self.start()

    def cleanup(self):
        for peer in self.peers:
            peer.close()
        if self.process and self.process.poll() is None:
            self.process.terminate()
            self.process.communicate(timeout=4)

    def start(self):
        executable = 'worminald-preview' if self.version == 2 else 'worminald-x11'
        if self.version == 2:
            executable = os.environ.get('WORMINAL_TEST_PREVIEW_DAEMON', executable)
        self.latest = None
        self.process = subprocess.Popen([str(ROOT / executable), '--serve'], env=self.env,
                                        stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        self.peer = self.connect()

    def connect(self):
        endpoint = 'worminal-workspace-v2' if self.version == 2 else 'worminal-workspace'
        deadline = time.monotonic() + 4
        while True:
            peer = socket.socket(socket.AF_UNIX)
            peer.settimeout(3)
            try:
                peer.connect(f'\0{endpoint}-{os.getuid()}-{self.scope}')
                self.peers.append(peer)
                return peer
            except OSError:
                peer.close()
                if time.monotonic() >= deadline:
                    self.fail('daemon did not listen')
                time.sleep(.01)

    def send(self, kind, tab=0, payload=b''):
        send(self.peer, kind, tab, payload, version=self.version)

    def catalog(self):
        # HELLO with an explicit command is used for v1, which has no ATTACH.
        if self.version == 2:
            with self.connect() as peer:
                send(peer, 19, version=2)
                _, data = wait_kind(peer, 7, version=2)
        else:
            deadline = time.monotonic() + 3
            while select.select([self.peer], [], [], .03)[0] or self.latest is None:
                kind, _, payload = packet(self.peer, version=1)
                if kind == 7:
                    self.latest = payload
                if time.monotonic() > deadline:
                    break
            data = self.latest
        def integer():
            nonlocal data
            value = struct.unpack('!I', data[:4])[0]
            data = data[4:]
            return value
        def text():
            nonlocal data
            size = integer()
            value, data = data[:size].decode(), data[size:]
            return value
        count = integer()
        integer(); integer()
        rows = []
        for _ in range(count):
            tab = integer()
            url = ''
            if self.version == 2:
                integer(); url = text()
            integer(); integer(); text()
            directory = text()
            for _ in range(integer()):
                text()
            rows.append((tab, directory, url))
        return rows

    def launch(self, args=()):
        payload = b''.join((number(80), number(24), number(1), number(len(args)), number(0),
                            string(''), string(str(self.default)), string(''), string(''), string(''),
                            *(string(arg) for arg in args)))
        self.latest = None
        self.send(1, payload=payload)
        if self.version == 2:
            tab, _ = wait_kind(self.peer, 20, version=2)
            return tab
        return self.catalog()[-1][0]

    def wait_directories(self, expected):
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            rows = self.catalog()
            if [row[1] for row in rows] == [str(path) for path in expected]:
                return rows
            time.sleep(.03)
        self.fail(f'expected directories {expected}, got {rows}')

    def restart(self, crash=False):
        self.process.send_signal(signal.SIGKILL if crash else signal.SIGTERM)
        self.process.communicate(timeout=4)
        self.start()

    @property
    def state(self):
        return self.root / 'state' / 'worminal' / f'tabs-v{self.version}-{self.scope.encode().hex()}'

    def wait_saved(self, predicate):
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            if self.state.exists() and predicate(self.state.read_bytes()):
                return
            time.sleep(.02)
        self.fail('recovery state was not committed')

    def wait_startup(self, text):
        deadline = time.monotonic() + 4
        path = self.root / 'startup'
        while time.monotonic() < deadline:
            if path.exists() and text in path.read_text():
                return path.read_text()
            time.sleep(.02)
        self.fail('shell did not finish its startup configuration')

    def test_directories_survive_shutdown_and_crash_without_replaying_commands(self):
        first, second = self.root / "first ' space", self.root / 'second\nline'
        first.mkdir(); second.mkdir()
        replay = self.root / 'replayed'
        one = self.launch(('/bin/sh', '-c', f'touch {shlex.quote(str(replay))}; exec /bin/zsh -d'))
        self.send(3, one, f'cd -- {shlex.quote(str(first))}\n'.encode())
        self.wait_directories([first])
        two = self.launch(('/bin/zsh', '-d'))
        self.send(3, two, f'cd -- {shlex.quote(str(second))}\n'.encode())
        self.wait_directories([first, second])
        self.wait_saved(lambda data: str(first).encode() in data and str(second).encode() in data)
        replay.unlink()
        for crash in (False, True):
            self.restart(crash)
            # The first ordinary launch reuses the restored workspace.
            self.launch()
            self.wait_directories([first, second])
            self.assertFalse(replay.exists(), 'restoration replayed a command')
        log = self.wait_startup(f'{second}|1\n')
        self.assertIn(f'{first}|1\n', log)
        self.assertIn(f'{second}|1\n', log)
        self.assertEqual(self.state.stat().st_mode & 0o777, 0o600)

    def test_missing_directory_keeps_tab_with_home_fallback(self):
        missing = self.root / 'removed'
        missing.mkdir()
        tab = self.launch(('/bin/zsh', '-d'))
        self.send(3, tab, f'cd -- {shlex.quote(str(missing))}\n'.encode())
        self.wait_directories([missing])
        self.wait_saved(lambda data: str(missing).encode() in data)
        self.process.terminate(); self.process.communicate(timeout=4)
        missing.rmdir()
        self.start(); self.launch()
        self.wait_directories([Path.home()])

    def test_closed_tabs_stay_closed(self):
        tab = self.launch(('/bin/zsh', '-d'))
        self.wait_directories([self.default])
        self.send(5, tab)
        self.wait_saved(lambda data: data == number(1) + number(0))
        self.restart()
        self.launch()
        self.wait_directories([self.default])
        self.assertNotIn('|1\n', self.wait_startup('|unset\n'))

    def test_corrupt_recovery_is_preserved_without_partial_restore(self):
        self.process.terminate(); self.process.communicate(timeout=4)
        invalid = number(1) + number(2) + string(str(self.default)) + string('') + b'broken'
        self.state.write_bytes(invalid)
        self.start(); self.launch()
        self.wait_directories([self.default])
        self.assertEqual(self.state.read_bytes(), invalid)
        self.assertNotIn('|1\n', self.wait_startup('|unset\n'))

    def test_browser_order_and_navigation_survive_restart(self):
        if self.version != 2:
            self.skipTest('browser records belong to v2')
        url = 'file:///example/a%20b.html?path=%2Ftest'
        self.send(17, payload=string('https://example.invalid/'))
        browser, _ = wait_kind(self.peer, 20, version=2)
        self.send(18, browser, string(url))
        self.launch(('/bin/zsh', '-d'))
        self.wait_saved(lambda data: url.encode() in data and data[4:8] == number(2))
        self.restart()
        self.launch()
        rows = self.catalog()
        self.assertEqual(len(rows), 2)
        self.assertEqual(rows[0][2], url)
        self.assertEqual(rows[1][1], str(self.default))


class LegacyRestoreTest(RestoreTest):
    version = 1
