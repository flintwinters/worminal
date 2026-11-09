"""Remote bootstrap and no-overwrite publication on private targets, never live hosts."""
import hashlib
import io
import json
import os
from pathlib import Path
import stat
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from runtime import ROOT, CHECKS
from tools import build, remote_install

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

    def test_platform_active_workspace_and_missing_python_fail_before_publication(self):
        for scenario in ('os','cpu','active','missing-python','runtime'):
            self.command(scenario, success=False)
            self.assertEqual(list(self.prefix.iterdir()), [])

    def test_help_is_discoverable_without_ssh(self):
        for args in (['--help'], ['api','help'], ['install-on-remote','--help']):
            result = subprocess.run([str(DAEMON),*args],env=self.env,capture_output=True,text=True,timeout=5)
            self.assertEqual(result.returncode,0,result.stderr)
            self.assertIn('install-on-remote',result.stdout)
