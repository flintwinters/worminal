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
from preview import profile


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

    def test_explorer_entries_drag_into_the_terminal(self):
        # The explorer page puts the entry's file URI on WebKit's drag; the terminal pastes it as a quoted path.
        with (isolated_display() as env, profile(env) as browser_env,
              tempfile.TemporaryDirectory(prefix='explorer-drop-', dir=CHECKS) as directory, ExitStack() as cleanup):
            # Its own scope keeps the explorer from forwarding into the terminal's owner as another tab.
            browser_env['WORMINAL_SHARED_SOCKET_SCOPE'] += '-explorer'
            for environment in (env, browser_env):
                environment.update(GDK_BACKEND='x11')
                environment.pop('WAYLAND_DISPLAY', None)
            title = 'Explorer drop terminal'
            app = subprocess.Popen([str(runtime_binary('worminal-rust')), '-T', title, '-o', '-',
                                    '-e', '/bin/sh', '-c', f"stty raw -echo; printf '\\033]2;{title} ready\\aR'; exec cat"],
                                   env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            cleanup.callback(stop_application, app)
            target = window(env, app)
            wait_title(env, target, title + ' ready')
            read_bytes(app.stdout, len(f'\033]2;{title} ready\aR'))
            path = Path(directory) / "it's here.txt"
            path.touch()
            explorer = (ROOT / 'file-explorer/dist/index.html').as_uri() + '?path=' + directory
            source = subprocess.Popen([str(runtime_binary('worminal-rust')), '--browser', explorer], env=browser_env,
                                      stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            cleanup.callback(stop_application, source)
            origin = window(browser_env, source)
            wait_title(browser_env, origin, directory)
            xdo = lambda *args: subprocess.run(['xdotool', *args], env=env, check=True)
            xdo('windowsize', origin, '700', '300')  # wide enough that the path stays on one line
            xdo('windowmove', origin, '0', '0')
            xdo('windowmove', target, '720', '100')
            time.sleep(.5)
            # The single entry's name starts the drag; the pointer then travels into the terminal.
            xdo('mousemove', '--window', origin, '30', '94', 'mousedown', '1')
            for x in range(40, 801, 20):
                xdo('mousemove', str(x), '200')
                time.sleep(.03)
            time.sleep(.2)
            xdo('mouseup', '1')
            expected = ("'" + str(path).replace("'", "'\\''") + "'").encode()
            self.assertEqual(read_bytes(app.stdout, len(expected)), expected)
