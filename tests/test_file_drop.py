"""Drag files from another GUI process into the actual terminal on a private display."""
from contextlib import ExitStack
from pathlib import Path
import select
import subprocess
import tempfile
import time
import unittest

from runtime import ROOT, CHECKS, runtime_binary, window, stop_application
from test_keys import read_bytes, wait_title
from x11 import isolated_display


class FileDropTest(unittest.TestCase):
    def test_external_file_drop_pastes_paths(self):
        for bracketed in (False, True):
            with (self.subTest(bracketed=bracketed), isolated_display() as env,
                  tempfile.TemporaryDirectory(prefix='file-drop-', dir=CHECKS) as directory, ExitStack() as cleanup):
                env.update(GDK_BACKEND='x11')
                env.pop('WAYLAND_DISPLAY', None)
                title = 'External file drop terminal'
                mode = b'\033[?2004h' if bracketed else b'\033[?2004l'
                mode += f'\033]2;{title} ready\a'.encode()
                escaped = ''.join(f'\\{byte:03o}' for byte in mode)
                app = subprocess.Popen([str(runtime_binary('worminal-rust')), '-T', title, '-o', '-',
                                        '-e', '/bin/sh', '-c',
                                        f"stty raw -echo; printf '{escaped}R'; exec cat"],
                                       env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                cleanup.callback(stop_application, app)
                target = window(env, app)
                wait_title(env, target, title + ' ready')
                self.assertEqual(read_bytes(app.stdout, len(mode) + 1), mode + b'R')
                paths = [Path(directory) / 'file with spaces', Path(directory) / "quote'$file"]
                for path in paths:
                    path.touch()
                source = subprocess.Popen(['/usr/bin/python3', str(ROOT / 'tests/file_drag_source.py'),
                                           *(path.as_uri() for path in paths)], env=env,
                                          stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                cleanup.callback(stop_application, source)
                origin = window(env, source)
                xdo = lambda *args: subprocess.run(['xdotool', *args], env=env, check=True)
                xdo('windowmove', target, '300', '100')
                xdo('windowmove', origin, '0', '0')
                time.sleep(.2)
                xdo('mousemove', '--window', origin, '50', '50', 'mousedown', '1')
                for x in range(60, 451, 30):
                    xdo('mousemove', str(x), '150')
                    time.sleep(.03)
                time.sleep(.2)
                xdo('mouseup', '1')
                expected = ' '.join("'" + str(path).replace("'", "'\\''") + "'" for path in paths).encode()
                if bracketed:
                    expected = b'\033[200~' + expected + b'\033[201~'
                actual = read_bytes(app.stdout, len(expected))
                source.terminate()
                output, error = source.communicate(timeout=5)
                self.assertEqual(actual, expected, (output, error))
                self.assertFalse(select.select([app.stdout], [], [], .1)[0], 'drop submitted extra input')
