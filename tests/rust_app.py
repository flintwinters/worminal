"""Exercise the actual Rust application with private PTYs, windows, tabs and reattachment."""
from runtime import CHECKS

from runtime import runtime_binary

from contextlib import ExitStack
import os
import fcntl
import json
from pathlib import Path
import socket
import signal
import shutil
import shlex
import tempfile
import subprocess
import termios
import time
import unittest
from unittest.mock import patch

os.environ['WORMINAL_FRONTEND'] = 'worminal-rust'
from test_keys import NavigationKeysTest, read_bytes, read_through, wait_title
from visible_latency import image_bytes
from workspace_protocol import send, wait_kind, catalog_records, service_pid
from x11 import isolated_display, close_window
from preview import wayland_display
from test_window_state import private_bus, WindowStateTest
from test_window_activation import WindowActivationTests
from test_notifications import NotificationTests
from runtime import copy_runtime, window, stop_application
from rust_module import RustModuleTest
from test_runtime import RuntimeCleanupTest
from test_file_drop import FileDropTest
from rust_app_split import SplitPaneTest
from test_master_detach import MasterDetachTest
from test_layout_restore import LayoutRestoreTest

ROOT = Path(__file__).resolve().parent.parent


def panel_shown(window):
    """Whether a 900px-wide window shows the side panel. Its right edge is one faint column between equal
    backgrounds, colored by the palette; rows through empty terminal and panel space carry no other such column."""
    from Xlib import X, error
    try:
        row = image_bytes(window.get_image(0, 250, 900, 1, X.ZPixmap, 0xffffffff))
    except error.BadMatch:
        return False
    color = lambda x: row[x * 4:x * 4 + 3]
    return any(color(x - 1) == color(x + 1) != color(x) for x in range(60, 600))


class RustApplicationTest(unittest.TestCase):

    def test_empty_attach_can_open_its_first_terminal(self):
        with isolated_display() as env:
            env.update(GDK_BACKEND='x11', SHELL='/bin/sh')
            env.pop('WAYLAND_DISPLAY', None)
            app = subprocess.Popen([str(runtime_binary("worminal-rust")), '--attach'], env=env,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            self.addCleanup(lambda: self.stop(app))
            wid = window(env, app)
            subprocess.run(['xdotool', 'windowfocus', '--sync', wid], env=env, check=True)
            subprocess.run(['xdotool', 'key', '--clearmodifiers', 'ctrl+shift+t'], env=env, check=True)
            with socket.socket(socket.AF_UNIX) as observer:
                observer.settimeout(5)
                observer.connect(f"\0worminal-workspace-v2-{os.getuid()}-{env['WORMINAL_SHARED_SOCKET_SCOPE']}")
                send(observer, 19, version=2)
                deadline = time.monotonic() + 5
                while not catalog_records(wait_kind(observer, 7, version=2)[1]):
                    self.assertLess(time.monotonic(), deadline, 'first tab did not open from empty attachment')
            self.assertIsNone(app.poll())

    def test_replacement_reclaims_idle_view_without_keyboard_input(self):
        from Xlib import X, display
        with (tempfile.TemporaryDirectory(prefix='rust-app-upgrade-', dir=CHECKS) as directory,
              isolated_display() as env, ExitStack() as cleanup):
            env.update(GDK_BACKEND='x11', SHELL='/bin/sh')
            env.pop('WAYLAND_DISPLAY', None)
            base = Path(directory)
            copy_runtime(base)
            marker = "printf %b " + shlex.quote(r"\033[48;2;17;99;201m\033[H\033[2JAFTER_UPGRADE")
            command = "trap " + shlex.quote(marker) + " USR1; printf 'R:%s\\n' $$; while :; do sleep 1; done"
            app = subprocess.Popen([str(runtime_binary('worminal-rust', prefix=base)), '-T', 'Rust upgrade',
                                    '-o', '-', '-e', '/bin/sh', '-c',
                                    command], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            cleanup.callback(stop_application, app)
            wid = window(env, app)
            ready = read_through(app.stdout, b'\n')
            self.assertTrue(ready.startswith(b'R:'), ready)
            shell = int(ready[2:].strip())
            tty = os.readlink(f'/proc/{shell}/fd/0')
            server = display.Display(env['DISPLAY'])
            try:
                root = server.create_resource_object('window', int(wid))
                def glyph():
                    geometry = root.get_geometry()
                    # Private Xvfb returns BGRX pixels. Sample the terminal body, away from chrome/cursor.
                    data = image_bytes(root.get_image(geometry.width // 2, geometry.height // 2, 1, 1, X.ZPixmap, 0xffffffff))
                    return tuple(data[:3][::-1])
                before = glyph()
                self.assertNotEqual(before, (17,99,201))
                def peer():
                    connection = socket.socket(socket.AF_UNIX); connection.settimeout(2)
                    connection.connect(f"\0worminal-workspace-v2-{os.getuid()}-{env['WORMINAL_SHARED_SOCKET_SCOPE']}")
                    return connection
                with peer() as connection: original = service_pid(connection)
                shutil.copy2(runtime_binary("worminald"), base / 'replacement')
                (base / 'replacement').replace(runtime_binary('worminald', prefix=base))
                deadline = time.monotonic() + 8
                while True:
                    try:
                        with peer() as connection: current = service_pid(connection)
                    except (OSError, AssertionError):
                        self.assertLess(time.monotonic(), deadline)
                        time.sleep(.02)
                        continue
                    if current != original: break
                    self.assertLess(time.monotonic(), deadline, 'private daemon did not replace itself')
                    time.sleep(.02)
                os.kill(shell, signal.SIGUSR1)
                while glyph() != (17,99,201):
                    self.assertIsNone(app.poll())
                    self.assertLess(time.monotonic(), deadline, 'idle Rust view did not paint replacement output')
                    time.sleep(.02)
                self.assertEqual(os.readlink(f'/proc/{shell}/fd/0'), tty)
            finally:
                server.close()

    def test_native_wayland_maps_and_paints_live_shell_output(self):
        with wayland_display() as env:
            self.assertNotIn('DISPLAY', env)
            env['WORMINAL_TRACE_TIMING'] = '1'
            path = CHECKS / "rust-app-wayland.log"
            with path.open('wb') as log:
                app = subprocess.Popen([str(runtime_binary("worminal-rust")), '-T', 'Rust Wayland', '-o', '-',
                                        '-e', '/bin/sh', '-c',
                                        "trap 'printf WAYLAND_VISIBLE' USR1; printf 'R:%s\\n' $$; "
                                        "while :; do sleep 1; done"],
                                       env=env, stdout=subprocess.PIPE, stderr=log)
                try:
                    ready = read_through(app.stdout, b'\n')
                    self.assertTrue(ready.startswith(b'R:'), ready)
                    shell = int(ready[2:].strip())
                    started = time.monotonic_ns()
                    os.kill(shell, signal.SIGUSR1)
                    self.assertEqual(read_bytes(app.stdout, len(b'WAYLAND_VISIBLE')), b'WAYLAND_VISIBLE')
                    deadline = time.monotonic() + 5
                    while True:
                        events = [line.split() for line in path.read_text().splitlines()
                                  if line.startswith('worminal-timing ')]
                        frames = [int(event[2]) for event in events if event[1] == 'frame' and int(event[2]) > started]
                        paints = [int(event[2]) for event in events if event[1] == 'paint']
                        if frames and paints and paints[-1] >= frames[-1]: break
                        self.assertIsNone(app.poll(), path.read_text())
                        self.assertLess(time.monotonic(), deadline, path.read_text())
                        time.sleep(.02)
                    self.assertIsNone(app.poll())
                finally:
                    if app.poll() is None: app.terminate()
                    app.communicate(timeout=5)
                    subprocess.run([str(runtime_binary("worminald")), '--stop'], env=env,
                                   stdout=subprocess.DEVNULL, stderr=log, timeout=5)

    def test_tabs_ctrl_n_and_shell_survival_after_window_close(self):
        with isolated_display() as env:
            env.update(GDK_BACKEND='x11', SHELL='/bin/sh', G_DEBUG='fatal-criticals')
            env.pop('WAYLAND_DISPLAY', None)
            executable = str(runtime_binary("worminal-rust"))
            app = subprocess.Popen([executable, '-T', 'Rust vertical', '-o', '-', '-e', '/bin/sh', '-c',
                                    "stty raw -echo; printf 'R:%s\\n' $$; exec cat"],
                                   env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            self.addCleanup(lambda: self.stop(app))
            wid = window(env, app)
            initial = read_through(app.stdout, b'\n')
            self.assertTrue(initial.startswith(b'R:'), initial)
            shell = int(initial[2:].strip())
            tty = os.readlink(f'/proc/{shell}/fd/0')
            subprocess.run(['xdotool', 'windowfocus', '--sync', wid], env=env, check=True)
            subprocess.run(['xdotool', 'key', '--clearmodifiers', 'ctrl+n'], env=env, check=True)
            self.assertEqual(read_bytes(app.stdout, 1), b'\x0e')
            with open(f'/proc/{shell}/fd/0', 'rb', buffering=0) as slave:
                before = fcntl.ioctl(slave, termios.TIOCGWINSZ, bytes(8))
                subprocess.run(['xdotool', 'windowsize', wid, '840', '500'], env=env, check=True)
                deadline = time.monotonic() + 3
                while fcntl.ioctl(slave, termios.TIOCGWINSZ, bytes(8)) == before:
                    self.assertLess(time.monotonic(), deadline, 'GUI resize did not reach the live PTY')
                    time.sleep(.01)
            with socket.socket(socket.AF_UNIX) as observer:
                observer.settimeout(3)
                observer.connect(f"\0worminal-workspace-v2-{os.getuid()}-{env['WORMINAL_SHARED_SOCKET_SCOPE']}")
                send(observer, 19, version=2)
                self.assertEqual(len(catalog_records(wait_kind(observer, 7, version=2)[1])), 1)
                subprocess.run(['xdotool', 'key', '--clearmodifiers', 'ctrl+shift+t'], env=env, check=True)
                deadline = time.monotonic() + 5
                while True:
                    records = catalog_records(wait_kind(observer, 7, version=2)[1])
                    if len(records) == 2: break
                    self.assertLess(time.monotonic(), deadline)
                # The observer's catalog can arrive before GTK handles its own CREATED/catalog.
                # Select the first tab only after the GUI has actually selected the newly created one.
                deadline = time.monotonic() + 5
                while subprocess.check_output(['xdotool', 'getwindowname', wid], env=env,
                                              text=True).strip() != 'Worminal':
                    self.assertLess(time.monotonic(), deadline, 'GUI did not select its new tab')
                    time.sleep(.01)
                subprocess.run(['xdotool', 'key', '--clearmodifiers', 'alt+1'], env=env, check=True)
                deadline = time.monotonic() + 5
                while subprocess.check_output(['xdotool', 'getwindowname', wid], env=env,
                                              text=True).strip() != 'Rust vertical':
                    self.assertLess(time.monotonic(), deadline, 'tab shortcut did not return to the original shell')
                    time.sleep(.01)
                # Both directions wrap, using the same tab order as Ctrl+Tab.
                subprocess.run(['xdotool', 'mousemove', '--window', wid, '20', '80'], env=env, check=True)
                for button, expected in (('5', 'Worminal'), ('5', 'Rust vertical'),
                                         ('4', 'Worminal'), ('4', 'Rust vertical')):
                    subprocess.run(['xdotool', 'keydown', 'ctrl', 'click', button, 'keyup', 'ctrl'],
                                   env=env, check=True)
                    wait_title(env, wid, expected)
                subprocess.run(['xdotool', 'key', '--clearmodifiers', 'ctrl+shift+n'], env=env, check=True)
                deadline = time.monotonic() + 5
                while len(catalog_records(wait_kind(observer, 7, version=2)[1])) != 3:
                    self.assertLess(time.monotonic(), deadline, 'new Rust window did not create a tab')
                close_window(env, 'Worminal Rust')
                self.assertIsNone(app.poll(), 'closing one window terminated its sibling')
                close_window(env, 'Rust vertical')
                self.assertEqual(app.wait(timeout=5), 0, app.stderr.read().decode(errors='replace'))
                self.assertEqual(os.readlink(f'/proc/{shell}/fd/0'), tty)
                attached = subprocess.Popen([executable, '--attach', '-T', 'Rust attach'], env=env,
                                            stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                self.addCleanup(lambda: self.stop(attached))
                target = window(env, attached)
                subprocess.run(['xdotool', 'windowfocus', '--sync', target], env=env, check=True)
                # Printer streams stay bound to their originating client, independently of view ownership.
                # Explicit printer controls bind this new client before observing its raw stdout echo.
                subprocess.run(['xdotool', 'key', '--clearmodifiers', 'ctrl+Print', 'ctrl+Print'], env=env, check=True)
                subprocess.run(['xdotool', 'type', '--clearmodifiers', 'survives'], env=env, check=True)
                self.assertEqual(read_bytes(attached.stdout, 8), b'survives')
                self.assertEqual(os.readlink(f'/proc/{shell}/fd/0'), tty)

    def test_ctrl_click_types_a_worminal_input_link_once(self):
        for connection in ([], ['--master', 'key-test']):
            with self.subTest(connection=connection), isolated_display() as env:
                root = CHECKS / f'input-link-{os.getpid()}'
                shutil.rmtree(root, ignore_errors=True)
                root.mkdir(parents=True)
                self.addCleanup(shutil.rmtree, root, ignore_errors=True)
                typed = root / 'typed.txt'
                env.update(GDK_BACKEND='x11')
                env.pop('WAYLAND_DISPLAY', None)
                if connection:
                    env['PATH'] = str(ROOT / 'tests/fixtures/preview-master') + os.pathsep + env['PATH']
                    env['WORMINAL_TEST_DAEMON'] = str(runtime_binary('worminald'))
                link = r'\033]8;;worminal://input?text=echo%%20once%%0A\033\\RUN-ONCE-LINK\033]8;;\033\\\n'
                # After the window settles, fill the screen with the link so a click in the terminal lands on one.
                script = (f"sleep 1; i=0; while [ $i -lt 80 ]; do printf '{link}'; i=$((i+1)); done; "
                          f"exec cat > {shlex.quote(str(typed))}")
                app = subprocess.Popen([str(runtime_binary('worminal-rust')), *connection, '-T', 'Rust input link',
                                        '-e', '/bin/sh', '-c', script],
                                       env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                try:
                    wid = window(env, app)
                    subprocess.run(['xdotool', 'windowfocus', '--sync', wid], env=env, check=True)
                    time.sleep(2)
                    subprocess.run(['xdotool', 'mousemove', '--window', wid, '30', '250', 'click', '1'],
                                   env=env, check=True)
                    subprocess.run(['xdotool', 'keydown', 'ctrl', 'mousedown', '1', 'keyup', 'ctrl',
                                    'mouseup', '1'], env=env, check=True)
                    time.sleep(.25)
                    self.assertEqual(typed.read_text(), '', 'clicks without Ctrl held throughout must not type')
                    subprocess.run(['xdotool', 'keydown', 'ctrl', 'click', '1', 'keyup', 'ctrl'],
                                   env=env, check=True)
                    deadline = time.monotonic() + 5
                    while 'echo once' not in (typed.read_text() if typed.exists() else ''):
                        self.assertLess(time.monotonic(), deadline, 'clicking the link typed nothing')
                        time.sleep(.05)
                    time.sleep(1)
                    self.assertEqual(typed.read_text(), 'echo once\n', 'one click typed the link text twice')
                finally:
                    self.stop(app)

    def test_starring_a_tab_bookmarks_its_directory(self):
        from Xlib import X, display
        with isolated_display() as env, ExitStack() as cleanup:
            # Never the real configuration: bookmarks.yaml is the user's own file.
            root = CHECKS / f'bookmarks-{os.getpid()}'
            shutil.rmtree(root, ignore_errors=True)
            cwd = root / 'work dir'
            cwd.mkdir(parents=True)
            self.addCleanup(shutil.rmtree, root, ignore_errors=True)
            saved = root / 'config/worminal/bookmarks.yaml'
            saved.parent.mkdir(parents=True)
            # Unbookmarked tabs show no star by default; this window shows them so the star can be clicked.
            (saved.parent / 'worminal.yaml').write_text('worminal: {unbookmarked_stars: true}\n')
            env.update(GDK_BACKEND='x11', XDG_CONFIG_HOME=str(root / 'config'))
            env.pop('WAYLAND_DISPLAY', None)
            sizes = root / 'sizes.txt'
            record = f"trap 'stty size >> {shlex.quote(str(sizes))}' WINCH; while :; do sleep 0.2; done"
            app = subprocess.Popen([str(runtime_binary('worminal-rust')), '-T', 'Rust bookmarks', '-o', '-',
                                    '-e', '/bin/sh', '-c', record],
                                   env=env, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            self.addCleanup(lambda: self.stop(app))
            wid = window(env, app)
            xdo = lambda *args: subprocess.run(['xdotool', *args], env=env, check=True)
            api = lambda *args: subprocess.check_output([str(runtime_binary('worminald')), 'api', *args],
                                                        env=env, text=True)
            xdo('windowfocus', '--sync', wid)

            def bookmarked():
                return saved.read_text() if saved.exists() else ''

            def wait_for(condition, message):
                deadline = time.monotonic() + 8
                while not condition():
                    self.assertIsNone(app.poll())
                    self.assertLess(time.monotonic(), deadline, message)
                    time.sleep(.05)
            # The strip stars only tabs whose directory is known; the daemon reports it shortly after start.
            wait_for(lambda: json.loads(api('list'))['tabs'][0].get('directory') == str(cwd),
                     'the tab never reported its directory')
            xdo('windowsize', wid, '900', '500')
            time.sleep(1.5)
            server = display.Display(env['DISPLAY'])
            cleanup.callback(server.close)
            window_object = server.create_resource_object('window', int(wid))

            # A status line under the panes takes a row from the terminal while it shows, and the shell sees the
            # screen shrink and grow again: a bounce. The shell records its size on every resize signal.
            resizes = lambda: sizes.read_text().count('\n') if sizes.exists() else 0
            quiet = resizes()
            # Ctrl+Shift+D bookmarks the tab, and again removes it.
            xdo('key', '--clearmodifiers', 'ctrl+shift+d')
            wait_for(lambda: f'directory: {cwd}' in bookmarked(), 'ctrl+shift+d did not bookmark the tab')
            time.sleep(1.5)
            self.assertEqual(resizes(), quiet, 'bookmarking resized the terminal: a status line bounced the screen')
            self.assertEqual(saved.stat().st_mode & 0o777, 0o600)
            xdo('key', '--clearmodifiers', 'ctrl+shift+d')
            wait_for(lambda: str(cwd) not in bookmarked() and saved.exists(), 'ctrl+shift+d did not remove it')
            # The star sits in the tab's left padding; a click there toggles without starting a drag or
            # selecting anything else.
            xdo('mousemove', '--window', wid, '5', '8', 'click', '1')
            wait_for(lambda: f'directory: {cwd}' in bookmarked(), 'clicking the star did not bookmark the tab')
            xdo('mousemove', '--window', wid, '5', '8', 'click', '1')
            # Quick clicks arrive as a press and a double-click event; only the press may toggle.
            wait_for(lambda: str(cwd) not in bookmarked(), 'clicking the star again did not remove it')
            time.sleep(1.5)
            self.assertEqual(resizes(), quiet, 'clicking the star resized the terminal')
            # `worminal --bookmark` from the tab's shell bookmarks its directory; repeating it never removes it.
            for _ in range(2):
                subprocess.run([str(runtime_binary('worminal-rust')), '--bookmark'], env=env, cwd=cwd,
                               check=True, timeout=10)
                wait_for(lambda: f'directory: {cwd}' in bookmarked(), 'worminal --bookmark did not bookmark')
            self.assertEqual(len(json.loads(api('list'))['tabs']), 1)

    def test_a_server_is_typed_into_the_panel_and_opens_a_tab_that_runs_ssh_then_a_shell(self):
        self.server_flow()

    def test_the_same_server_flow_when_sh_is_bash(self):
        # Many systems have bash as sh, and a shell exits when its foreground child dies of SIGINT unless it is
        # interactive and traps the signal: without that, Ctrl+C on a connecting ssh closes the tab under bash.
        bash = shutil.which('bash')
        if not bash:
            self.skipTest('bash is not installed')
        self.server_flow(interpreter=bash)

    def server_flow(self, interpreter=None):
        import re
        from Xlib import X, display, error
        with isolated_display() as env, ExitStack() as cleanup:
            root = CHECKS / f'servers-{os.getpid()}-{"bash" if interpreter else "default"}'
            shutil.rmtree(root, ignore_errors=True)
            fakes = root / 'bin'
            fakes.mkdir(parents=True)
            self.addCleanup(shutil.rmtree, root, ignore_errors=True)
            # A fake ssh: `ssh 30` sleeps for 30 seconds, `ssh bad` prints sleep's complaint and exits.
            (fakes / 'ssh').symlink_to('/bin/sleep')
            if interpreter:
                # The launcher runs `sh` from PATH, so this decides which shell interprets the ssh script.
                (fakes / 'sh').symlink_to(interpreter)
            saved = root / 'config/worminal/bookmarks.yaml'
            env.update(GDK_BACKEND='x11', XDG_CONFIG_HOME=str(root / 'config'), SHELL='/bin/sh',
                       PATH=f"{fakes}:{env['PATH']}")
            env.pop('WAYLAND_DISPLAY', None)
            app = subprocess.Popen([str(runtime_binary('worminal-rust')), '-T', 'Rust servers', '-o', '-',
                                    '-e', '/bin/sh', '-c', 'while :; do sleep 1; done'],
                                   env=env, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            self.addCleanup(lambda: self.stop(app))
            wid = window(env, app)
            xdo = lambda *args: subprocess.run(['xdotool', *args], env=env, check=True)
            api = lambda *args: subprocess.check_output([str(runtime_binary('worminald')), 'api', *args],
                                                        env=env, text=True)
            server = display.Display(env['DISPLAY'])
            cleanup.callback(server.close)
            window_object = server.create_resource_object('window', int(wid))
            xdo('windowfocus', '--sync', wid)
            xdo('windowsize', wid, '900', '500')
            time.sleep(1.5)

            panel_open = lambda: panel_shown(window_object)

            def wait_for(condition, message):
                deadline = time.monotonic() + 10
                while not condition():
                    self.assertIsNone(app.poll())
                    self.assertLess(time.monotonic(), deadline, message() if callable(message) else message)
                    time.sleep(.05)
            text = lambda: saved.read_text() if saved.exists() else ''
            tabs = lambda: json.loads(api('list'))['tabs']
            key = lambda *names: xdo('key', '--clearmodifiers', *names)
            type_text = lambda value: xdo('type', '--delay', '40', value)
            wait_for(lambda: len(tabs()) == 1, 'no first tab')
            key('ctrl+shift+b')
            wait_for(panel_open, 'ctrl+shift+b did not open the panel')
            # `a` asks for a server. Something that cannot be one is refused and nothing is saved, and the list's
            # own keys (a, j, x, q) are plain letters while typing.
            key('a')
            type_text('x;y')
            key('Return')
            time.sleep(.8)
            self.assertNotIn('host', text(), 'a malformed server was saved')
            self.assertTrue(panel_open(), 'a refused entry closed the panel')
            key('Escape')            # cancels the question only ...
            time.sleep(.5)
            self.assertTrue(panel_open(), 'Escape while typing closed the whole panel')
            # ... then a real one. `bad` contains the add key itself.
            key('a')
            type_text('bad')
            key('Return')
            # The host is accepted, then it asks what to call it; nothing is saved until that is answered, and
            # names (unlike hosts) may contain spaces and the list's keys (r, a).
            time.sleep(.8)
            self.assertNotIn('host', text(), 'the server was saved before it was named')
            type_text('Bad box')
            key('Return')
            wait_for(lambda: re.search(r"host: ['\"]?bad['\"]?", text()), 'the typed server was not saved')
            self.assertIn('title: Bad box', text(), 'the name was not saved with the server')
            self.assertNotIn('- directory:', text())   # (the header comment mentions the word)
            key('Return')            # opens the selected row: a tab that runs ssh bad
            wait_for(lambda: len(tabs()) == 2, 'opening the server did not create a tab')
            wait_for(lambda: not panel_open(), 'opening a server left the panel open')
            bad = tabs()[1]['id']
            # ssh failed at once, yet the tab stays and shows why: it is a shell now, not a closed tab.
            # Text a tab prints in its first few milliseconds, before the window has attached and sized it, ends up in
            # scrollback rather than on the visible screen (any tab the window creates does this), so read retained rows.
            screen = lambda: ' '.join(json.loads(api('read', str(bad), '--tail', '100'))['results'][0]['lines']).strip()
            tab_command = lambda tab: Path(f"/proc/{[t for t in tabs() if t['id'] == tab][0]['shell_pid']}/cmdline").read_bytes()
            wait_for(lambda: 'invalid time interval' in screen(),
                     lambda: f'no error on screen; it shows {screen()!r}; the tab runs {tab_command(bad)}')
            time.sleep(2)
            self.assertEqual(len(tabs()), 2, 'the tab closed when ssh ended')
            type_text(f"echo up > {root / 'up.txt'}")
            key('Return')
            wait_for(lambda: (root / 'up.txt').exists(), 'no shell took over after ssh ended')
            # A server that stays connected: it is a tab running ssh, so it must not be bookmarkable as a
            # local directory (the daemon has to see ssh in front, which is what set -m is for).
            key('ctrl+shift+b')
            wait_for(panel_open, 'the panel did not reopen')
            key('a')
            type_text('30')
            key('Return')
            key('Return')            # no name
            wait_for(lambda: re.search(r"host: ['\"]?30['\"]?", text()), 'the second server was not saved')
            self.assertEqual(text().count('title:'), 1, 'an unnamed server got a name')
            # `r` names the selected row, starting from its current name; here from nothing.
            key('Down')
            key('r')
            type_text('Thirty')
            key('Return')
            wait_for(lambda: 'title: Thirty' in text(), 'renaming did not save the name')
            # Renaming again starts from the old name, and a blank answer clears it.
            key('r')
            key('ctrl+u')
            key('Return')
            wait_for(lambda: 'Thirty' not in text(), 'a blank name did not clear the name')
            self.assertIn('title: Bad box', text(), 'renaming changed another bookmark')
            key('Return')
            wait_for(lambda: len(tabs()) == 3, 'the second server did not open')
            time.sleep(2.5)          # the daemon notices ssh in front on its next check
            before = text()
            key('ctrl+shift+d')
            time.sleep(1.5)
            self.assertEqual(text(), before, 'a tab running ssh was bookmarked as a local directory')
            # Ending ssh leaves the shell, not a closed tab.
            key('ctrl+c')
            time.sleep(1)
            type_text(f"echo back > {root / 'back.txt'}")
            key('Return')
            wait_for(lambda: (root / 'back.txt').exists(), 'no shell took over after ssh was interrupted')
            self.assertEqual(len(tabs()), 3)

    def test_a_server_bookmark_with_a_port_runs_ssh_dash_p_and_without_one_leaves_ssh_its_default(self):
        from Xlib import X, display, error
        with isolated_display() as env, ExitStack() as cleanup:
            root = CHECKS / f'server-ports-{os.getpid()}'
            shutil.rmtree(root, ignore_errors=True)
            (root / 'bin').mkdir(parents=True)
            self.addCleanup(shutil.rmtree, root, ignore_errors=True)
            # A fake ssh records exactly the arguments it was given, then stays connected.
            arguments = root / 'arguments'
            fake = root / 'bin/ssh'
            fake.write_text(f'#!/bin/sh\nprintf "%s\\n" "$@" > {arguments}\nsleep 30\n')
            fake.chmod(0o755)
            saved = root / 'config/worminal/bookmarks.yaml'
            env.update(GDK_BACKEND='x11', XDG_CONFIG_HOME=str(root / 'config'), SHELL='/bin/sh',
                       PATH=f"{root / 'bin'}:{env['PATH']}")
            env.pop('WAYLAND_DISPLAY', None)
            app = subprocess.Popen([str(runtime_binary('worminal-rust')), '-T', 'Rust ports', '-o', '-',
                                    '-e', '/bin/sh', '-c', 'while :; do sleep 1; done'],
                                   env=env, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            self.addCleanup(lambda: self.stop(app))
            wid = window(env, app)
            xdo = lambda *args: subprocess.run(['xdotool', *args], env=env, check=True)
            key = lambda *names: xdo('key', '--clearmodifiers', *names)
            type_text = lambda value: xdo('type', '--delay', '40', value)
            xdo('windowfocus', '--sync', wid)
            xdo('windowsize', wid, '900', '500')
            time.sleep(1.5)

            def wait_for(condition, message):
                deadline = time.monotonic() + 10
                while not condition():
                    self.assertIsNone(app.poll())
                    self.assertLess(time.monotonic(), deadline, message)
                    time.sleep(.05)

            def add(server, down):
                key('ctrl+shift+b')
                time.sleep(.8)
                key('a')
                type_text(server)
                key('Return')
                time.sleep(.5)
                key('Return')        # no name
                time.sleep(.5)
                for _ in range(down):
                    key('Down')
                arguments.unlink(missing_ok=True)
                key('Return')        # opens the selected row
                wait_for(arguments.exists, f'ssh never ran for {server}')
                time.sleep(.3)
                return arguments.read_text().split()

            self.assertEqual(add('me@build:2222', 0), ['-p', '2222', 'me@build'])
            self.assertIn('host: me@build', saved.read_text())
            self.assertIn('port: 2222', saved.read_text())
            # No port, and the default port, both leave ssh to choose (22, or what ~/.ssh/config says).
            self.assertEqual(add('plain@host', 1), ['plain@host'])
            self.assertEqual(add('other:22', 2), ['other'])
            text = saved.read_text()
            self.assertEqual(text.count('port:'), 1, text)
            self.assertNotIn('other:22', text)

    def test_a_tab_running_ssh_is_not_bookmarkable_as_a_local_directory(self):
        with isolated_display() as env:
            # The daemon reports an ssh tab's directory as its local shell's, so bookmarking it would save the
            # wrong place and light up every tab that shares that directory.
            root = CHECKS / f'ssh-bookmarks-{os.getpid()}'
            shutil.rmtree(root, ignore_errors=True)
            work = root / 'work'
            work.mkdir(parents=True)
            self.addCleanup(shutil.rmtree, root, ignore_errors=True)
            # A shell named ssh is a harmless foreground job the daemon takes for an ssh client.
            fake = root / 'ssh'
            fake.symlink_to('/bin/sh')
            saved = root / 'config/worminal/bookmarks.yaml'
            env.update(GDK_BACKEND='x11', XDG_CONFIG_HOME=str(root / 'config'), SHELL='/bin/sh')
            env.pop('WAYLAND_DISPLAY', None)
            app = subprocess.Popen([str(runtime_binary('worminal-rust')), '-T', 'Rust ssh', '-o', '-',
                                    '-e', str(fake), '-c', 'read line'],
                                   env=env, cwd=work, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            self.addCleanup(lambda: self.stop(app))
            wid = window(env, app)
            xdo = lambda *args: subprocess.run(['xdotool', *args], env=env, check=True)
            api = lambda *args: subprocess.check_output([str(runtime_binary('worminald')), 'api', *args],
                                                        env=env, text=True)
            xdo('windowfocus', '--sync', wid)
            bookmarked = lambda: saved.read_text() if saved.exists() else ''

            def wait_for(condition, message):
                deadline = time.monotonic() + 8
                while not condition():
                    self.assertIsNone(app.poll())
                    self.assertLess(time.monotonic(), deadline, message)
                    time.sleep(.05)
            tabs = lambda: json.loads(api('list'))['tabs']
            wait_for(lambda: tabs()[0].get('directory') == str(work), 'the ssh tab never reported its directory')
            time.sleep(2.5)  # the daemon notices the foreground ssh client on its next directory check
            xdo('key', '--clearmodifiers', 'ctrl+shift+d')
            time.sleep(1.5)
            self.assertNotIn(str(work), bookmarked(), 'an ssh tab was bookmarked as its local directory')
            # A second, ordinary tab in the same directory can be bookmarked.
            xdo('key', '--clearmodifiers', 'ctrl+shift+t')
            wait_for(lambda: len(tabs()) == 2 and tabs()[1].get('directory') == str(work), 'no second tab')
            time.sleep(1)
            xdo('key', '--clearmodifiers', 'ctrl+shift+d')
            wait_for(lambda: f'directory: {work}' in bookmarked(), 'the ordinary tab was not bookmarked')
            # Back on the ssh tab the same keys must leave that bookmark alone, not remove it.
            xdo('key', '--clearmodifiers', 'alt+1')
            time.sleep(1)
            xdo('key', '--clearmodifiers', 'ctrl+shift+d')
            time.sleep(1.5)
            self.assertIn(f'directory: {work}', bookmarked(), 'the ssh tab removed another tab\'s bookmark')

    def test_the_side_panel_opens_and_removes_bookmarks_from_the_keyboard(self):
        from Xlib import X, display, error
        with isolated_display() as env, ExitStack() as cleanup:
            root = CHECKS / f'panel-{os.getpid()}'
            shutil.rmtree(root, ignore_errors=True)
            start, other = root / 'start', root / 'other dir'
            start.mkdir(parents=True)
            other.mkdir()
            self.addCleanup(shutil.rmtree, root, ignore_errors=True)
            saved = root / 'config/worminal/bookmarks.yaml'
            saved.parent.mkdir(parents=True)
            saved.write_text(f'bookmarks:\n  - directory: "{other}"\n')
            env.update(GDK_BACKEND='x11', XDG_CONFIG_HOME=str(root / 'config'), SHELL='/bin/sh')
            env.pop('WAYLAND_DISPLAY', None)
            sizes = root / 'sizes.txt'
            record = f"trap 'stty size >> {shlex.quote(str(sizes))}' WINCH; while :; do sleep 0.2; done"
            app = subprocess.Popen([str(runtime_binary('worminal-rust')), '-T', 'Rust panel', '-o', '-',
                                    '-e', '/bin/sh', '-c', record],
                                   env=env, cwd=start, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            self.addCleanup(lambda: self.stop(app))
            wid = window(env, app)
            xdo = lambda *args: subprocess.run(['xdotool', *args], env=env, check=True)
            api = lambda *args: subprocess.check_output([str(runtime_binary('worminald')), 'api', *args],
                                                        env=env, text=True)
            server = display.Display(env['DISPLAY'])
            cleanup.callback(server.close)
            window_object = server.create_resource_object('window', int(wid))
            xdo('windowfocus', '--sync', wid)
            xdo('windowsize', wid, '900', '500')
            time.sleep(1.5)

            panel_edge = lambda: panel_shown(window_object)

            def wait_for(condition, message):
                deadline = time.monotonic() + 8
                while not condition():
                    self.assertIsNone(app.poll())
                    self.assertLess(time.monotonic(), deadline, message)
                    time.sleep(.05)
            tabs = lambda: json.loads(api('list'))['tabs']
            wait_for(lambda: tabs()[0].get('directory') == str(start), 'the tab never reported its directory')
            self.assertFalse(panel_edge(), 'the panel is open before it was asked for')
            quiet = sizes.read_text() if sizes.exists() else ''
            # Ctrl+Shift+B opens the panel (P is an alias for keyboards that can send it); Enter opens the selected bookmark in a new tab and closes it.
            xdo('key', '--clearmodifiers', 'ctrl+shift+b')
            wait_for(panel_edge, 'ctrl+shift+b did not open the panel')
            xdo('key', '--clearmodifiers', 'Return')
            wait_for(lambda: len(tabs()) == 2 and tabs()[1].get('directory') == str(other),
                     'Enter did not open the bookmark in a new tab at its directory')
            wait_for(lambda: not panel_edge(), 'opening a bookmark left the panel open')
            # The new terminal has the keyboard, and its shell started in the bookmarked directory.
            xdo('type', '--delay', '30', 'pwd > marker.txt')
            xdo('key', '--clearmodifiers', 'Return')
            wait_for(lambda: (other / 'marker.txt').exists() and (other / 'marker.txt').read_text().strip() == str(other),
                     'the opened tab did not take the keyboard in the bookmarked directory')
            self.assertEqual(sizes.read_text() if sizes.exists() else '', quiet,
                             'opening and closing the bookmarks panel resized the original terminal')
            # Escape closes the panel and gives the keyboard back; Delete removes the selected bookmark.
            xdo('key', '--clearmodifiers', 'ctrl+shift+b')
            wait_for(panel_edge, 'ctrl+shift+b did not reopen the panel')
            xdo('key', '--clearmodifiers', 'Escape')
            wait_for(lambda: not panel_edge(), 'Escape did not close the panel')
            xdo('key', '--clearmodifiers', 'ctrl+shift+p')
            wait_for(panel_edge, 'the ctrl+shift+p alias did not open the panel')
            xdo('key', '--clearmodifiers', 'Delete')
            wait_for(lambda: str(other) not in saved.read_text(), 'Delete did not remove the bookmark')
            self.assertEqual(len(tabs()), 2)

    def test_the_panel_lists_this_machine_first_and_selects_rows_across_the_remote_separator(self):
        from Xlib import X, display
        with isolated_display() as env, ExitStack() as cleanup:
            root = CHECKS / f'panel-split-{os.getpid()}'
            shutil.rmtree(root, ignore_errors=True)
            first, second = root / 'first', root / 'second'
            first.mkdir(parents=True)
            second.mkdir()
            self.addCleanup(shutil.rmtree, root, ignore_errors=True)
            saved = root / 'config/worminal/bookmarks.yaml'
            saved.parent.mkdir(parents=True)
            # The server is listed first in the file but shown after this machine's directories.
            saved.write_text(f'bookmarks:\n  - host: me@build\n  - directory: "{first}"\n  - directory: "{second}"\n')
            env.update(GDK_BACKEND='x11', XDG_CONFIG_HOME=str(root / 'config'), SHELL='/bin/sh')
            env.pop('WAYLAND_DISPLAY', None)
            app = subprocess.Popen([str(runtime_binary('worminal-rust')), '-T', 'Rust panel split', '-o', '-',
                                    '-e', '/bin/sh', '-c', 'while :; do sleep 1; done'],
                                   env=env, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            self.addCleanup(lambda: self.stop(app))
            wid = window(env, app)
            xdo = lambda *args: subprocess.run(['xdotool', *args], env=env, check=True)
            api = lambda *args: subprocess.check_output([str(runtime_binary('worminald')), 'api', *args],
                                                        env=env, text=True)
            server = display.Display(env['DISPLAY'])
            cleanup.callback(server.close)
            window_object = server.create_resource_object('window', int(wid))
            xdo('windowfocus', '--sync', wid)
            xdo('windowsize', wid, '900', '500')
            time.sleep(1.5)
            tabs = lambda: json.loads(api('list'))['tabs']

            def wait_for(condition, message):
                deadline = time.monotonic() + 8
                while not condition():
                    self.assertIsNone(app.poll())
                    self.assertLess(time.monotonic(), deadline, message)
                    time.sleep(.05)
            wait_for(lambda: tabs()[0].get('directory') == str(root), 'the tab never reported its directory')
            xdo('key', '--clearmodifiers', 'ctrl+shift+b')
            wait_for(lambda: panel_shown(window_object), 'the panel did not open')
            # Row 1 is the second directory: the server did not take row 0 of the file's order.
            xdo('key', '--clearmodifiers', 'Down', 'Return')
            wait_for(lambda: len(tabs()) == 2 and tabs()[1].get('directory') == str(second),
                     'the second row did not open the second directory')

    @staticmethod
    def stop(process):
        stop_application(process)


def main():
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument('-k')
    match = parser.parse_args().k
    if match:
        unittest.defaultTestLoader.testNamePatterns = [f'*{match}*']
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(RustApplicationTest)
    suite.addTests(unittest.defaultTestLoader.loadTestsFromTestCase(SplitPaneTest))
    suite.addTests(unittest.defaultTestLoader.loadTestsFromTestCase(FileDropTest))
    suite.addTests(unittest.defaultTestLoader.loadTestsFromTestCase(MasterDetachTest))
    suite.addTests(unittest.defaultTestLoader.loadTestsFromTestCase(LayoutRestoreTest))
    suite.addTests(unittest.defaultTestLoader.loadTestsFromTestCase(WindowStateTest))
    suite.addTests(unittest.defaultTestLoader.loadTestsFromTestCase(WindowActivationTests))
    suite.addTests(unittest.defaultTestLoader.loadTestsFromTestCase(RustModuleTest))
    suite.addTests(unittest.defaultTestLoader.loadTestsFromTestCase(RuntimeCleanupTest))
    for name in ('test_navigation_keys', 'test_alternate_scroll_sends_cursor_keys',
                 'test_clipboard_paste_bytes', 'test_ctrl_end_in_micro', 'test_master_navigation_keys',
                 'test_master_alternate_scroll_sends_cursor_keys', 'test_master_clipboard_paste_bytes'):
        if not match or match in name:
            suite.addTest(NavigationKeysTest(name))
    with (private_bus(os.environ) as env,
          patch.dict(os.environ, {'DBUS_SESSION_BUS_ADDRESS': env['DBUS_SESSION_BUS_ADDRESS']})):
        return 0 if unittest.TextTestRunner(verbosity=1).run(suite).wasSuccessful() else 1


if __name__ == '__main__':
    raise SystemExit(main())
