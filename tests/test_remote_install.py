"""Remote bootstrap and no-overwrite publication on private targets, never live hosts."""
import hashlib
import io
import json
import os
from pathlib import Path
import stat
import shutil
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

    def test_help_is_discoverable_without_ssh(self):
        for args in (['--help'], ['api','help'], ['install-on-remote','--help']):
            result = subprocess.run([str(DAEMON),*args],env=self.env,capture_output=True,text=True,timeout=5)
            self.assertEqual(result.returncode,0,result.stderr)
            self.assertIn('install-on-remote',result.stdout)
