"""Check the native frontend ABI and lazy launcher independently of application widgets."""
import ctypes
import errno
import os
from pathlib import Path
import select
import socket
import subprocess
import tempfile
import unittest
import uuid

from rust_runtime import ROOT, RUST_FRONTEND_MODULE, RUST_FRONTEND_SYMBOL, copy_runtime
from x11 import isolated_display


class RustModuleTest(unittest.TestCase):
    def test_gui_broker_rejects_foreign_credentials(self):
        from rust_runtime import window, stop_application
        with (isolated_display() as env,
              tempfile.TemporaryDirectory(prefix='rust-peer-', dir=ROOT / '.checks') as directory):
            executable = copy_runtime(directory)
            env.update(GDK_BACKEND='x11', LD_PRELOAD=str(ROOT / 'build/gui-peer.so'))
            app = subprocess.Popen([str(executable), '--attach', '-T', 'Credential owner'], env=env,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            try:
                window(env, app)
                result = subprocess.run([str(executable), '-e', '/bin/cat'], env=env,
                                        capture_output=True, timeout=5)
                self.assertNotEqual(result.returncode, 0, 'foreign GUI peer was accepted')
                self.assertIsNone(app.poll())
                windows = subprocess.check_output(['xdotool', 'search', '--onlyvisible', '--pid', str(app.pid)],
                                                  env=env, text=True).splitlines()
                self.assertEqual(len(windows), 1)
            finally:
                stop_application(app)

    def test_invalid_display_starts_neither_local_daemon_nor_ssh(self):
        with tempfile.TemporaryDirectory(prefix='rust-invalid-display-', dir=ROOT / '.checks') as directory:
            executable = copy_runtime(directory)
            base = Path(directory)
            # Both helpers are inert sentinels: a failed display must never reach either launch operation.
            for name in ('worminald', 'ssh'):
                program = base / name
                program.write_text('#!/bin/sh\ntouch "${0}.started"\nexit 1\n'); program.chmod(0o755)
            env = {**os.environ, 'DISPLAY': 'worminal-invalid', 'WAYLAND_DISPLAY': 'worminal-invalid',
                   'GDK_BACKEND': 'x11', 'WORMINAL_SHARED_SOCKET_SCOPE': 'invalid-' + uuid.uuid4().hex,
                   'PATH': str(base) + os.pathsep + os.environ['PATH']}
            for args in (['--attach'], ['--master', 'preview-test']):
                result = subprocess.run([str(executable), *args], env=env, capture_output=True, timeout=5)
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertIn(b'cannot open display', result.stderr.lower())
                self.assertFalse(list(base.glob('*.started')), 'failed display started a helper')

    def test_invalid_module_arguments_release_the_transferred_listener(self):
        module = ctypes.CDLL(str(ROOT / 'build' / RUST_FRONTEND_MODULE))
        with self.assertRaises(AttributeError): getattr(module, 'worminal_frontend_v1')
        entry = getattr(module, RUST_FRONTEND_SYMBOL)
        entry.argtypes = [ctypes.c_int32, ctypes.POINTER(ctypes.c_char_p), ctypes.c_int32, ctypes.c_uint32]
        entry.restype = ctypes.c_int32
        with tempfile.TemporaryDirectory(prefix='rust-module-abi-', dir=ROOT / '.checks') as directory:
            for index, (argc, arguments, flags, expected) in enumerate(((0, None, 1, 1), (1, [None], 1, 1),
                    (2, [b'worminal-rust', b'--unsupported-module-option'], 1, 2),
                    (1, [b'worminal-rust'], 0x80000000, 1))):
                listener = socket.socket(socket.AF_UNIX)
                listener.bind(str(Path(directory) / f'listener-{index}')); listener.listen()
                descriptor = listener.detach()
                argv = None if arguments is None else (ctypes.c_char_p * len(arguments))(*arguments)
                self.assertEqual(entry(argc, argv, descriptor, flags), expected)
                with self.assertRaises(OSError) as error: os.fstat(descriptor)
                self.assertEqual(error.exception.errno, errno.EBADF)

    def test_graphics_load_only_in_owner_and_forwarding_does_not_need_the_module(self):
        from rust_runtime import window, stop_application
        dependencies = subprocess.check_output(['readelf', '-d', str(ROOT / 'worminal-rust')], text=True)
        for library in ('libgtk', 'libgdk', 'libpango', 'libcairo', 'libwebkit'):
            self.assertNotIn(library, dependencies, f'lightweight launcher links {library}')
        with (isolated_display() as env,
              tempfile.TemporaryDirectory(prefix='rust-module-launch-', dir=ROOT / '.checks') as directory):
            env.update(GDK_BACKEND='x11', SHELL='/bin/sh'); env.pop('WAYLAND_DISPLAY', None)
            executable = copy_runtime(directory)
            app = subprocess.Popen([str(executable), '--attach', '-T', 'Module owner'], env=env,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            try:
                window(env, app)
                module = Path(directory) / 'build' / RUST_FRONTEND_MODULE
                maps = Path(f'/proc/{app.pid}/maps').read_text()
                self.assertIn(str(module), maps)
                self.assertNotIn('libwebkit2gtk', maps)
                # A forwarded process must exit without loading graphics or resolving module paths.
                module.rename(module.with_suffix('.hidden'))
                result = subprocess.run([str(executable), '-T', 'Forwarded through owner', '-e', '/bin/cat'],
                                        env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=5)
                self.assertEqual(result.returncode, 0, result.stderr.decode(errors='replace'))
                self.assertEqual(result.stdout, b'', 'ordinary forwarded launch retained its caller output pipe')
                # The shared owner maps both windows; the short-lived launcher creates no replacement process.
                windows = subprocess.check_output(['xdotool', 'search', '--onlyvisible', '--pid', str(app.pid)],
                                                  env=env, text=True).splitlines()
                self.assertEqual(len(windows), 2)
                # An explicit later launch recovers a stopped service in the existing GUI owner,
                # after GTK has initialized. Its replacement service must not need a frontend reload.
                from workspace_protocol import service_pid
                from test_keys import read_bytes
                with socket.socket(socket.AF_UNIX) as observer:
                    observer.settimeout(3)
                    observer.connect(f"\0worminal-workspace-v2-{os.getuid()}-{env['WORMINAL_SHARED_SOCKET_SCOPE']}")
                    previous_pid = service_pid(observer)
                identity = os.pidfd_open(previous_pid)
                try:
                    subprocess.run([str(executable.with_name('worminald')), '--stop'], env=env,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=True, timeout=3)
                    self.assertTrue(select.select([identity], [], [], 5)[0], 'private service did not stop')
                finally:
                    os.close(identity)
                recovered = subprocess.Popen([str(executable), '-T', 'Recovered daemon', '-o', '-',
                    '-e', '/bin/sh', '-c', 'printf RECOVERED; exec cat'], env=env,
                    stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                try:
                    self.assertEqual(recovered.wait(timeout=5), 0)
                    self.assertEqual(read_bytes(recovered.stdout, 9), b'RECOVERED')
                finally:
                    if recovered.poll() is None:
                        recovered.terminate()
                        recovered.wait(timeout=5)
                    recovered.stdout.close(); recovered.stderr.close()
            finally:
                stop_application(app)
