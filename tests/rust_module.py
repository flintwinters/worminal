"""Check the native frontend ABI and lazy launcher independently of application widgets."""
import ctypes
import errno
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import unittest

from rust_runtime import ROOT, RUST_FRONTEND_MODULE, copy_runtime
from x11 import isolated_display


class RustModuleTest(unittest.TestCase):
    def test_invalid_module_arguments_release_the_transferred_listener(self):
        module = ctypes.CDLL(str(ROOT / 'build' / RUST_FRONTEND_MODULE))
        with self.assertRaises(AttributeError): getattr(module, 'worminal_frontend_v1')
        entry = module.worminal_rust_frontend_v1
        entry.argtypes = [ctypes.c_int32, ctypes.POINTER(ctypes.c_char_p), ctypes.c_int32]
        entry.restype = ctypes.c_int32
        with tempfile.TemporaryDirectory(prefix='rust-module-abi-', dir=ROOT / '.checks') as directory:
            for index, (argc, arguments, expected) in enumerate(((0, None, 1), (1, [None], 1),
                    (2, [b'worminal-rust', b'--unsupported-module-option'], 2))):
                listener = socket.socket(socket.AF_UNIX)
                listener.bind(str(Path(directory) / f'listener-{index}')); listener.listen()
                descriptor = listener.detach()
                argv = None if arguments is None else (ctypes.c_char_p * len(arguments))(*arguments)
                self.assertEqual(entry(argc, argv, descriptor), expected)
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
                                        env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, timeout=5)
                self.assertEqual(result.returncode, 0, result.stderr.decode(errors='replace'))
                # The shared owner maps both windows; the short-lived launcher creates no replacement process.
                windows = subprocess.check_output(['xdotool', 'search', '--onlyvisible', '--pid', str(app.pid)],
                                                  env=env, text=True).splitlines()
                self.assertEqual(len(windows), 2)
            finally:
                stop_application(app)
