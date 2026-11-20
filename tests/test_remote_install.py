"""Remote bootstrap and no-overwrite publication on private targets, never live hosts."""
import hashlib
import io
import json
import os
from pathlib import Path
import stat
import shutil
import select
import signal
import socket
import time
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from runtime import ROOT, CHECKS
from tools import build, remote_install

MODULES = ('frontend-rust-v2.so', 'browser-rust-v2.so')
DAEMON = Path(os.environ.get('WORMINAL_TEST_PREVIEW_DAEMON', str(ROOT / 'build/cargo/release/worminal-daemon')))


def payload(files):
    rows = [{'role': role, 'size': len(data), 'sha256': hashlib.sha256(data).hexdigest()}
            for role, data in files]
    return (json.dumps({'files': rows})+'\n').encode() + b''.join(data for _, data in files)


class PublicationTest(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix='remote-publish-', dir=CHECKS)
        self.addCleanup(self.directory.cleanup)
        self.prefix = Path(self.directory.name)
        context = patch.object(remote_install, 'PREFIX', self.prefix)
        context.start()
        self.addCleanup(context.stop)
        self.layout = build.HEADLESS_INSTALL
        self.files = [('daemon', b'candidate-daemon'), ('manual', b'candidate-manual')]

    def targets(self):
        return [path for _, path in remote_install.destinations(self.layout)]

    def test_publish_is_private_atomic_and_never_replaces_existing_files(self):
        remote_install.publish(self.layout, io.BytesIO(payload(self.files)))
        for (item, path), (_, data) in zip(remote_install.destinations(self.layout), self.files):
            self.assertEqual(path.read_bytes(), data)
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), item['mode'])
        with self.assertRaises(ValueError):
            remote_install.publish(self.layout, io.BytesIO(payload(self.files)))
        self.assertEqual(self.targets()[0].read_bytes(), self.files[0][1])

    def test_truncated_or_corrupt_transfers_publish_nothing(self):
        for data in (payload(self.files)[:-1], payload(self.files).replace(b'candidate-daemon', b'changed!!-daemon')):
            with self.assertRaises(ValueError):
                remote_install.publish(self.layout, io.BytesIO(data))
            self.assertTrue(all(not path.exists() for path in self.targets()))
            self.assertEqual(list(self.prefix.rglob('.worminal-install-*')), [])

    def test_privileged_destination_cannot_follow_a_symlink(self):
        outside = self.prefix / 'outside'
        outside.mkdir()
        (self.prefix / 'bin').symlink_to(outside, target_is_directory=True)
        with self.assertRaises(OSError):
            remote_install.publish(self.layout, io.BytesIO(payload(self.files)))
        self.assertEqual(list(outside.iterdir()), [])

    def test_only_the_trusted_group_may_share_write_access_to_a_destination_directory(self):
        (self.prefix / 'bin').mkdir()
        (self.prefix / 'bin').chmod(0o775)
        with self.assertRaisesRegex(ValueError, 'writable by another user'):
            remote_install.publish(self.layout, io.BytesIO(payload(self.files)))
        with patch.object(remote_install, 'TRUSTED_GROUP', os.getegid()):
            remote_install.publish(self.layout, io.BytesIO(payload(self.files)))
        self.assertEqual(self.targets()[0].read_bytes(), self.files[0][1])

    def test_concurrent_destination_is_preserved_and_our_partial_publication_is_removed(self):
        original = os.link
        def race(source, target, **kwargs):
            if target == self.targets()[1].name:
                fd = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600,
                             dir_fd=kwargs['dst_dir_fd'])
                os.write(fd, b'concurrent-owner')
                os.close(fd)
            return original(source, target, **kwargs)
        with patch.object(remote_install.os, 'link', side_effect=race), self.assertRaises(FileExistsError):
            remote_install.publish(self.layout, io.BytesIO(payload(self.files)))
        self.assertFalse(self.targets()[0].exists())
        self.assertEqual(self.targets()[1].read_bytes(), b'concurrent-owner')


    def test_replace_swaps_files_atomically_and_requires_an_installed_daemon(self):
        with self.assertRaises(ValueError):
            remote_install.publish(self.layout, io.BytesIO(payload(self.files)), True)
        remote_install.publish(self.layout, io.BytesIO(payload(self.files)))
        newer = [('daemon', b'newer-daemon'), ('manual', b'newer-manual')]
        before = self.targets()[0].stat().st_ino
        remote_install.publish(self.layout, io.BytesIO(payload(newer)), True)
        self.assertEqual([path.read_bytes() for path in self.targets()], [data for _, data in newer])
        # A new inode is what the running daemon's executable watch needs to hand off live shells.
        self.assertNotEqual(self.targets()[0].stat().st_ino, before)
        self.assertEqual(list(self.prefix.rglob('.worminal-install-*')), [])


    def test_replace_creates_promoted_desktop_files_with_modules_before_the_launcher(self):
        remote_install.publish(self.layout, io.BytesIO(payload(self.files)))
        desktop = [{'role': role, 'base': 'prefix', 'path': path, 'mode': 0o755}
                   for role, path in (('frontend', 'lib/worminal/f.so'), ('launcher', 'bin/worminal'))]
        layout = {**self.layout, 'files': self.layout['files'] + desktop}
        swapped = []
        original = os.rename
        def record(source, target, **kwargs):
            swapped.append(target)
            return original(source, target, **kwargs)
        files = self.files + [('frontend', b'module'), ('launcher', b'launcher')]
        with patch.object(remote_install.os, 'rename', side_effect=record):
            remote_install.publish(layout, io.BytesIO(payload(files)), True)
        self.assertEqual(swapped, ['worminal.1', 'f.so', 'worminal', 'worminald'])
        self.assertEqual((self.prefix/'bin/worminal').read_bytes(), b'launcher')


class RemoteCommandTest(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix='remote-command-', dir=CHECKS)
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.prefix = self.root / 'prefix'
        self.prefix.mkdir(mode=0o700)
        self.env = {**os.environ, 'PATH': str(self.root)+os.pathsep+os.environ['PATH'],
                    'WORMINAL_TEST_INSTALL_PREFIX': str(self.prefix), 'XDG_CACHE_HOME': str(self.root/'cache')}
        # Execute the actual embedded bootstrap against an isolated target prefix. sudo
        # is a local fixture: it runs the publisher as this test user, never escalates.
        (self.root/'sudo').write_text('#!/usr/bin/python3\nimport os,sys\n'
                                     'if sys.argv[1:]==["-n","true"]: sys.exit(0)\n'
                                     'assert sys.argv[1]=="-n"\n'
                                     'os.execv(sys.argv[2],sys.argv[2:])\n')
        (self.root/'sudo').chmod(0o700)
        (self.root/'ssh').write_text('#!/usr/bin/python3\nimport runpy\n'
                                    f'runpy.run_path({str(ROOT / "tests/remote_install_ssh.py")!r}, '
                                    'run_name="__main__")\n')
        (self.root/'ssh').chmod(0o700)
        # The pull direction runs the same bootstrap on this node; its python3 targets a private prefix too.
        self.local = self.root / 'local'
        self.local.mkdir(mode=0o700)
        self.env['WORMINAL_TEST_LOCAL_PREFIX'] = str(self.local)
        (self.root/'python3').write_text('#!/usr/bin/python3\nimport os,runpy\n'
                                        'os.environ["WORMINAL_TEST_INSTALL_PREFIX"]=os.environ["WORMINAL_TEST_LOCAL_PREFIX"]\n'
                                        f'runpy.run_path({str(ROOT / "tests/remote_install_ssh.py")!r}, '
                                        'run_name="__main__")\n')
        (self.root/'python3').chmod(0o700)

    def command(self, scenario='success', success=True):
        result = subprocess.run([str(DAEMON), 'install-on-remote', 'fixture-host'],
                                env={**self.env, 'WORMINAL_TEST_INSTALL_SCENARIO':scenario},
                                capture_output=True, text=True, timeout=15)
        self.assertEqual(result.returncode, 0 if success else 1, result.stderr + result.stdout)
        value = json.loads(result.stdout)
        self.assertEqual(value['ok'], success)
        return value

    def test_install_then_refuse_replacement_and_preserve_user_state(self):
        state = self.root/'saved-user-state'
        state.write_text('preserve')
        result = self.command()
        self.assertTrue(result['installed'])
        self.assertFalse(result['started'])
        self.assertEqual((self.prefix/'bin/worminald').read_bytes(), DAEMON.read_bytes())
        self.assertTrue((self.prefix/'share/man/man1/worminal.1').is_file())
        value = self.command(success=False)
        self.assertIn('already exists', value['error'])
        self.assertEqual(state.read_text(), 'preserve')

    def graphical_controller(self):
        """A copy of the daemon installed beside a launcher and modules, as `manage.py install` lays them out."""
        controller = self.root / 'controller'
        for directory in ('bin', 'lib/worminal'):
            (controller/directory).mkdir(parents=True)
        shutil.copy(DAEMON, controller/'bin/worminald')
        # Like the real launcher, even --version fails unless the frontend module sits at ../lib/worminal
        # (subtle: staging the launcher alone on a target without modules once silently dropped the desktop files).
        (controller/'bin/worminal').write_text(
            f'#!/bin/sh\n[ -f "$(dirname "$0")/../lib/worminal/{MODULES[0]}" ] || exit 1\n'
            '[ "$1" = --version ] && echo worminal\nexit 0\n')
        (controller/'bin/worminal').chmod(0o755)
        for module in MODULES:
            (controller/'lib/worminal'/module).write_bytes(b'module ' + module.encode())
        return controller/'bin/worminald'

    def install_from(self, daemon, graphics, command='install-on-remote'):
        result = subprocess.run([str(daemon), command, 'fixture-host'],
                                env={**self.env, 'WORMINAL_TEST_GRAPHICS': '1' if graphics else '0'},
                                capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        value = json.loads(result.stdout)
        self.assertTrue(value['installed'], value)
        return value

    def test_graphical_target_gets_the_worminal_command_beside_the_daemon(self):
        daemon = self.graphical_controller()
        result = self.install_from(daemon, graphics=True)
        self.assertTrue(result['desktop'])
        for name in ('bin/worminal', 'share/applications/worminal.desktop',
                     'share/icons/hicolor/scalable/apps/worminal.svg'):
            self.assertTrue((self.prefix/name).is_file(), name)
        for module in MODULES:
            self.assertEqual((self.prefix/'lib/worminal'/module).read_bytes(), b'module ' + module.encode())
        self.assertEqual((self.prefix/'bin/worminald').read_bytes(), daemon.read_bytes())

    def test_headless_target_or_headless_controller_installs_only_the_daemon(self):
        self.assertFalse(self.install_from(self.graphical_controller(), graphics=False)['desktop'])
        self.assertFalse((self.prefix/'bin/worminal').exists())
        shutil.rmtree(self.prefix); self.prefix.mkdir(mode=0o700)
        self.assertFalse(self.install_from(DAEMON, graphics=True)['desktop'])
        self.assertFalse((self.prefix/'bin/worminal').exists())

    def test_broken_launcher_keeps_the_daemon_install(self):
        daemon = self.graphical_controller()
        (daemon.parent/'worminal').write_text('#!/bin/sh\nexit 1\n')
        result = self.install_from(daemon, graphics=True)
        self.assertFalse(result['desktop'])
        self.assertTrue((self.prefix/'bin/worminald').is_file())
        self.assertFalse((self.prefix/'bin/worminal').exists())

    def test_older_headless_peer_with_graphics_is_promoted_by_the_push(self):
        daemon = self.graphical_controller()
        self.fake(self.prefix, '0.0.1')
        self.assertTrue(self.install_from(daemon, graphics=True, command='upgrade-remote')['desktop'])
        self.assertTrue((self.prefix/'bin/worminal').is_file())

    def test_older_peer_desktop_is_replaced_by_the_push(self):
        # Version sync updates the whole installation, so a peer's GUI never lags its daemon.
        daemon = self.graphical_controller()
        self.fake(self.prefix, '0.0.1')
        (self.prefix/'bin/worminal').write_text('#!/bin/sh\necho old\n')
        self.assertTrue(self.install_from(daemon, graphics=True, command='upgrade-remote')['desktop'])
        self.assertEqual((self.prefix/'bin/worminal').read_bytes(), (daemon.parent/'worminal').read_bytes())
        self.assertEqual((self.prefix/'lib/worminal'/MODULES[0]).read_bytes(), b'module ' + MODULES[0].encode())

    def fake(self, prefix, version):
        """An installed daemon stand-in: reports its version (or predates --version) and passes the run check."""
        for directory in ('bin', 'share', 'share/man', 'share/man/man1'):
            (prefix/directory).mkdir(mode=0o755, exist_ok=True)
        report = f'echo "worminald {version}"' if version else 'exit 2'
        (prefix/'bin/worminald').write_text(f'#!/bin/sh\nif [ "$1" = --version ]; then {report}; else echo Worp; fi\n')
        (prefix/'bin/worminald').chmod(0o755)
        (prefix/'share/man/man1/worminal.1').write_text(f'manual {version}')

    def upgrade(self):
        result = subprocess.run([str(DAEMON), 'upgrade-remote', 'fixture-host'], env=self.env,
                                capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        return json.loads(result.stdout)

    def test_older_peer_is_pushed_our_daemon(self):
        for version in (None, '0.0.1'):
            self.fake(self.prefix, version)
            result = self.upgrade()
            self.assertEqual((result['installed'], result['direction']), (True, 'push'))
            self.assertEqual((self.prefix/'bin/worminald').read_bytes(), DAEMON.read_bytes())
        self.assertTrue(self.upgrade()['current'])

    def test_newer_peer_updates_this_node_instead(self):
        self.fake(self.prefix, '99.0.0')
        for local in (None, '0.0.1'):
            self.fake(self.local, local)
            result = self.upgrade()
            self.assertEqual((result['installed'], result['direction']), (True, 'pull'))
            for name in ('bin/worminald', 'share/man/man1/worminal.1'):
                self.assertEqual((self.local/name).read_bytes(), (self.prefix/name).read_bytes())
        self.assertTrue(self.upgrade()['current'])

    def test_equal_peer_and_missing_installation_change_nothing(self):
        self.fake(self.prefix, subprocess.run([str(DAEMON), '--version'], capture_output=True, text=True).stdout.split()[-1])
        before = (self.prefix/'bin/worminald').read_bytes()
        self.assertTrue(self.upgrade()['current'])
        self.assertEqual((self.prefix/'bin/worminald').read_bytes(), before)
        (self.prefix/'bin/worminald').unlink()
        result = subprocess.run([str(DAEMON), 'upgrade-remote', 'fixture-host'], env=self.env,
                                capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 1)
        self.assertFalse(json.loads(result.stdout)['ok'])

    def test_platform_active_workspace_and_missing_python_fail_before_publication(self):
        for scenario in ('os','cpu','active','missing-python','runtime'):
            self.command(scenario, success=False)
            self.assertEqual(list(self.prefix.iterdir()), [])

    def connect_environment(self):
        from workspace_protocol import stop_workspace
        env = {**self.env, 'WORMINAL_SHARED_SOCKET_SCOPE': self.root.name,
                'XDG_STATE_HOME': str(self.root/'state'), 'XDG_CONFIG_HOME': str(self.root/'config'),
                'WORMINAL_TEST_SSH_TRACE': str(self.root/'ssh-trace')}
        self.addCleanup(stop_workspace, env)
        return env

    def test_authenticated_connect_finishes_update_before_forwarding_queued_view_input(self):
        from workspace_protocol import send, wait_kind, stop_workspace
        env = self.connect_environment()
        gate = self.root/'update-gate'
        env['WORMINAL_TEST_UPDATE_GATE'] = str(gate)
        # Exercise both publication and an equal-version no-op. Each connection must authenticate
        # and check versions before starting the bridge, even when the previous one succeeded.
        self.fake(self.prefix, '0.0.1')
        for _ in range(2):
            peer, child = socket.socketpair()
            peer.settimeout(10)
            with child:
                process = subprocess.Popen([str(DAEMON), '--connect-master', 'fixture-host', '--bridge'],
                                           env=env, stdin=child, stdout=child, stderr=subprocess.PIPE)
            try:
                send(peer, 19, version=2)  # queued before authentication/update finishes
                deadline = time.monotonic()+10
                while not gate.with_suffix('.waiting').exists() and time.monotonic()<deadline:
                    self.assertIsNone(process.poll())
                    time.sleep(.01)
                self.assertTrue(gate.with_suffix('.waiting').exists())
                self.assertEqual((self.prefix/'bin/worminald').read_bytes(), DAEMON.read_bytes())
                self.assertFalse(select.select([peer], [], [], .1)[0], 'View received bytes before update finished')
                with socket.socket(socket.AF_UNIX) as probe:
                    with self.assertRaises(ConnectionRefusedError):
                        probe.connect(f'\0worminal-workspace-v2-{os.getuid()}-{self.root.name}')
                gate.touch()
                try:
                    wait_kind(peer, 7, version=2)
                except AssertionError as error:
                    _, detail = process.communicate(timeout=5)
                    self.fail(f'{error}; {detail.decode(errors="replace")}')
            finally:
                gate.touch()  # unblock the fixture before waiting, including on assertion failure
                peer.close()
                try:
                    _, error = process.communicate(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.communicate(timeout=5)
                    raise
                stop_workspace(env)
            self.assertEqual(process.returncode, 0, error)
            gate.unlink()
            gate.with_suffix('.waiting').unlink()
        self.assertEqual((self.root/'ssh-trace').read_text().splitlines(), ['authenticated']*2,
                         'Version/config/bridge must share one authenticated SSH connection')

    def test_closing_a_view_during_update_reaps_its_authenticated_ssh_child(self):
        self.fake(self.prefix, '0.0.1')
        gate, pid_file = self.root/'update-gate', self.root/'ssh-pid'
        env = {**self.connect_environment(), 'WORMINAL_TEST_UPDATE_GATE': str(gate),
               'WORMINAL_TEST_SSH_PID': str(pid_file)}
        process = subprocess.Popen([str(DAEMON), '--connect-master', 'fixture-host', '--bridge'],
                                   env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        identity = None
        try:
            deadline = time.monotonic()+10
            while not gate.with_suffix('.waiting').exists() and time.monotonic()<deadline:
                self.assertIsNone(process.poll())
                time.sleep(.01)
            self.assertTrue(gate.with_suffix('.waiting').exists())
            identity = os.pidfd_open(int(pid_file.read_text()))
            process.terminate()
            process.wait(timeout=5)
            self.assertTrue(select.select([identity], [], [], 5)[0],
                            'Closing the view orphaned its authenticated SSH connection')
        finally:
            gate.touch()
            if identity is not None:
                try:
                    signal.pidfd_send_signal(identity, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                self.assertTrue(select.select([identity], [], [], 5)[0])
                os.close(identity)
            if process.poll() is None:
                process.kill()
            process.communicate(timeout=5)

    def test_failed_update_never_opens_a_workspace_or_exposes_bootstrap_json_to_the_view(self):
        report, writer = os.pipe()
        self.addCleanup(os.close, report)
        env = {**self.connect_environment(), 'WORMINAL_TEST_INSTALL_SCENARIO': 'runtime',
               'WORMINAL_CONNECT_REPORT_FD': str(writer)}
        self.fake(self.prefix, '0.0.1')
        try:
            result = subprocess.run([str(DAEMON), '--connect-master', 'fixture-host', '--bridge'], pass_fds=(writer,),
                                    env=env, input=b'queued view input', capture_output=True, timeout=15)
        finally:
            os.close(writer)
        # The view notifies the desktop from this report; a closed stream alone does not say why.
        self.assertRegex(os.read(report, 4096), rb'^Version sync with fixture-host failed: .*staged daemon cannot run')
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, b'')
        self.assertIn(b'staged daemon cannot run', result.stderr)
        self.assertFalse((self.root/'state').exists())
        self.assertEqual((self.root/'ssh-trace').read_text().splitlines(), ['authenticated'])

    def test_authenticated_existing_connect_never_starts_a_missing_workspace(self):
        env = self.connect_environment()
        self.fake(self.prefix, '0.0.1')
        shutil.copyfile(DAEMON, self.prefix/'bin/worminald')
        peer, child = socket.socketpair()
        peer.settimeout(10)
        with child:
            process = subprocess.Popen([str(DAEMON), '--connect-master', 'fixture-host', '--bridge-existing'],
                                       env=env, stdin=child, stdout=child, stderr=subprocess.PIPE)
        with peer:
            try:
                self.assertEqual(peer.recv(1), b'')
                _, error = process.communicate(timeout=5)
            finally:
                if process.poll() is None:
                    process.kill()
                    process.communicate(timeout=5)
        self.assertIn(b"Connection refused", error)
        self.assertEqual(list((self.root/'state/worminal').glob('tabs-v2-*')), [])
        self.assertEqual((self.root/'ssh-trace').read_text().splitlines(), ['authenticated'])

    def test_help_is_discoverable_without_ssh(self):
        for args in (['--help'], ['api','help'], ['install-on-remote','--help']):
            result = subprocess.run([str(DAEMON),*args],env=self.env,capture_output=True,text=True,timeout=5)
            self.assertEqual(result.returncode,0,result.stderr)
            self.assertIn('install-on-remote',result.stdout)
