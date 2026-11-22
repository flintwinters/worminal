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
from workspace_protocol import send, wait_kind, catalog_records, service_pid
from x11 import isolated_display, close_window
from preview import profile, service, wayland_display
from test_window_state import private_bus, WindowStateTest
from test_window_activation import WindowActivationTests
from test_notifications import NotificationTests
from runtime import copy_runtime, window, stop_application
from rust_module import RustModuleTest
from test_runtime import RuntimeCleanupTest
from test_file_drop import FileDropTest
from test_master_detach import MasterDetachTest

ROOT = Path(__file__).resolve().parent.parent


def window_size(root):
    geometry = root.get_geometry()
    return geometry.width, geometry.height


def fix_size(root, xdo, wid, width, height):
    """Resize once the first terminal has fitted the window to its character grid; returns the settled size."""
    def settle():
        last, since = None, time.monotonic()
        while time.monotonic() - since < .5:
            if window_size(root) != last:
                last, since = window_size(root), time.monotonic()
            time.sleep(.02)
        return last
    settle()
    xdo('windowsize', wid, str(width), str(height))
    return settle()


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
                    data = root.get_image(geometry.width // 2, geometry.height // 2, 1, 1, X.ZPixmap, 0xffffffff).data
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

    def test_dragging_a_tab_onto_a_pane_edge_splits_the_window(self):
        from Xlib import X, display, error
        from PIL import Image
        red, blue = (200, 30, 30), (20, 40, 210)
        # Each terminal repaints its whole screen continuously: resizing keeps text but not erased background.
        # Typing through the agent API would take the window's control of the tab, so no tab is typed into.
        fill = lambda color, title: ("while :; do printf '\\033]2;%s\\007\\033[48;2;%d;%d;%dm\\033[2J'; sleep .2; done"
                                     % ((title,) + color))
        with isolated_display() as env, ExitStack() as cleanup:
            script = CHECKS / 'split-shell.sh'
            script.write_text(f'#!/bin/sh\n{fill(blue, "blue")}\n')
            script.chmod(0o755)
            # Tabs carry a star in their left padding: drags start on the label, and the config is private.
            config = CHECKS / f'split-config-{os.getpid()}'
            self.addCleanup(shutil.rmtree, config, ignore_errors=True)
            env.update(GDK_BACKEND='x11', SHELL=str(script), XDG_CONFIG_HOME=str(config))
            env.pop('WAYLAND_DISPLAY', None)
            app = subprocess.Popen([str(runtime_binary("worminal-rust")), '-T', 'Rust split', '-o', '-',
                                    '-e', '/bin/sh', '-c', fill(red, 'red')],
                                   env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            self.addCleanup(lambda: self.stop(app))
            wid = window(env, app)
            xdo = lambda *args: subprocess.run(['xdotool', *args], env=env, check=True)
            api = lambda *args: subprocess.check_output([str(runtime_binary('worminald')), 'api', *args],
                                                        env=env, text=True)
            server = display.Display(env['DISPLAY'])
            cleanup.callback(server.close)
            root = server.create_resource_object('window', int(wid))

            size = lambda: window_size(root)
            self.assertEqual(fix_size(root, xdo, wid, 900, 500), (900, 500))

            def pixel(fx, fy):
                try:
                    data = root.get_image(int(size()[0] * fx), int(size()[1] * fy), 1, 1,
                                          X.ZPixmap, 0xffffffff).data
                except error.BadMatch:  # not mapped yet
                    return None
                return tuple(data[:3][::-1])

            def wait_for(left, right, message):
                deadline = time.monotonic() + 8
                while (pixel(.25, .75), pixel(.75, .75)) != (left, right):
                    self.assertIsNone(app.poll())
                    if time.monotonic() >= deadline:
                        width, height = size()
                        data = root.get_image(0, 0, width, height, X.ZPixmap, 0xffffffff).data
                        Image.frombuffer('RGB', (width, height), data, 'raw', 'BGRX', 0, 1).save(
                            CHECKS / 'rust-app-split.png')
                        self.fail(f'{message}: {pixel(.25, .75)} {pixel(.75, .75)}')
                    time.sleep(.05)

            wait_for(red, red, 'first tab did not fill the window')
            xdo('windowfocus', '--sync', wid)
            xdo('key', '--clearmodifiers', 'ctrl+shift+t')
            deadline = time.monotonic() + 5
            while len(json.loads(api('list'))['tabs']) != 2:
                self.assertLess(time.monotonic(), deadline, 'second tab did not open')
                time.sleep(.05)
            # The new tab is selected and fills the window.
            wait_for(blue, blue, 'second tab did not fill the window')
            data = root.get_image(0, 0, 900, 500, X.ZPixmap, 0xffffffff).data
            Image.frombuffer('RGB', (900, 500), data, 'raw', 'BGRX', 0, 1).save(CHECKS / 'split-before.png')
            # Locate the selected second label rather than assuming a particular font
            # size or directory basename width. Its right edge must clear the first label.
            background = pixel(.49, 4 / 500)
            second_label = max(x for x in range(2, 440) if pixel(x / 900, 4 / 500) != background) - 2
            self.assertLess(second_label, 450, 'fixture labels must fit in one half-width strip')
            # Drag the first tab's label onto the right edge of the pane.
            xdo('mousemove', '--window', wid, '40', '8', 'mousedown', '1')
            for step in range(1, 11):
                xdo('mousemove', '--window', wid, str(40 + 848 * step // 10), str(8 + 250 * step // 10))
                time.sleep(.02)
            deadline = time.monotonic() + 3
            while pixel(.75, .75) == blue:
                self.assertLess(time.monotonic(), deadline, 'split target highlight did not paint over content')
                time.sleep(.02)
            self.assertEqual(pixel(.25, .75), blue, 'split target highlight covered the wrong half')
            # A translucent copy of the tab follows the pointer (at 888, 258), held where it was grabbed: just
            # left of the pointer lies the tab's padding, which differs from the highlighted content around it.
            under = lambda: pixel(880 / 900, 258 / 500)
            marked = pixel(.75, .75)
            deadline = time.monotonic() + 3
            while under() == marked:
                self.assertLess(time.monotonic(), deadline, 'no dragged tab image under the pointer')
                time.sleep(.02)
            self.assertNotEqual(pixel(880 / 900, 300 / 500), under(), 'the dragged tab image is too tall')
            xdo('mouseup', '1')
            # The anchor stays left; the dropped tab takes the right half and focus.
            wait_for(blue, red, 'dropping a tab on the right edge did not split the window')
            self.assertEqual(under(), red, 'the dragged tab image outlived the drop')
            # Split panes are outlined, the focused one (right, just dropped) brighter than the other. Look at the
            # inner edges: the window's own outer frame is drawn by the display, not by the panes.
            left_edge, right_edge = pixel(449.5 / 900, .75), pixel(451.5 / 900, .75)
            self.assertNotEqual(left_edge, blue, 'the left pane has no border')
            self.assertNotEqual(right_edge, red, 'the right pane has no border')
            self.assertGreater(sum(right_edge), sum(left_edge), f'the focused border is not brighter: {left_edge} {right_edge}')
            # Clicking the other pane makes its tab the current one, not just the target of typing: Ctrl+Shift+C/V
            # and the window title follow the app's active tab, which a click on an already-live pane once left alone.
            title = lambda: subprocess.check_output(['xdotool', 'getwindowname', wid], env=env, text=True).strip()
            self.assertEqual(title(), 'red')
            xdo('mousemove', '--window', wid, '225', '300', 'click', '1')
            deadline = time.monotonic() + 5
            while title() != 'blue':
                self.assertLess(time.monotonic(), deadline, f'clicking the left pane left {title()!r} current')
                time.sleep(.05)
            xdo('mousemove', '--window', wid, '675', '300', 'click', '1')
            deadline = time.monotonic() + 5
            while title() != 'red':
                self.assertLess(time.monotonic(), deadline, f'clicking the right pane left {title()!r} current')
                time.sleep(.05)
            # Each pane has its own strip holding only its tabs: the dropped tab is selected in the right strip,
            # and the left strip no longer has the second tab where the shared strip had it.
            self.assertNotEqual(pixel(.505, 4 / 500), background, 'the right pane has no tab strip of its own')
            self.assertEqual(pixel(second_label / 900, 4 / 500), background, 'the left strip still lists the dropped tab')
            # Dragging the right pane's tab onto the left strip puts it back there and ends the split. Over a strip
            # the drop is marked by the yellow slot line there, never by the pane highlight, which is for content.
            # The press stays right of the tab's star, which would bookmark instead of drag. The pointer ends near
            # the strip's top, so the tab image it drags leaves the line's lower end clear, and over the left half of the strip's tab, so the dropped tab goes before it and stays first.
            content = next(y for y in range(500) if pixel(.25, y / 500) == blue)
            xdo('mousemove', '--window', wid, '490', '8', 'mousedown', '1')
            for step in range(1, 11):
                xdo('mousemove', '--window', wid, str(490 - 484 * step // 10), str(8 - 5 * step // 10))
                time.sleep(.02)
            yellow = bytes.fromhex(json.loads((ROOT / 'assets/palette.json').read_text())['normal']['yellow'][1:])
            strip = lambda: root.get_image(0, 0, 450, content, X.ZPixmap, 0xffffffff).data
            deadline = time.monotonic() + 3
            while not any(bytes(data[i:i + 3][::-1]) == yellow for data in [strip()] for i in range(0, len(data), 4)):
                self.assertLess(time.monotonic(), deadline, 'no slot marker in the strip the tab is over')
                time.sleep(.02)
            self.assertEqual(pixel(.25, .75), blue, 'a drag over a strip highlighted the pane')
            xdo('mouseup', '1')
            wait_for(red, red, 'dropping a tab on the other strip did not merge the panes')
            self.assertEqual(pixel(.505, 4 / 500), background, 'the right strip outlived the split')
            # Split again, then Ctrl+Shift+X gathers the tabs.
            xdo('mousemove', '--window', wid, '40', '8', 'mousedown', '1')
            for step in range(1, 11):
                xdo('mousemove', '--window', wid, str(40 + 848 * step // 10), str(8 + 250 * step // 10))
                time.sleep(.02)
            xdo('mouseup', '1')
            wait_for(blue, red, 'the second split did not happen')
            xdo('key', '--clearmodifiers', 'ctrl+shift+x')
            wait_for(red, red, 'ctrl+shift+x did not leave only the focused pane')
            self.assertEqual(pixel(.505, 4 / 500), background, 'the right strip outlived the unsplit')
            # The keyboard does what dragging does. At the window's edge Alt+Shift+Right splits the shown
            # tab off to the right (the first tab is red and was the one selected by the gather) ...
            xdo('key', '--clearmodifiers', 'alt+shift+Right')
            wait_for(blue, red, 'alt+shift+Right did not split the tab off to the right')
            # ... Ctrl+Shift+Left focuses the left pane without moving anything ...
            xdo('key', '--clearmodifiers', 'ctrl+shift+Left')
            time.sleep(.3)
            wait_for(blue, red, 'ctrl+shift+Left moved a tab')
            # ... and with a pane to the right Alt+Shift+Right moves the tab into it, ending the split.
            xdo('key', '--clearmodifiers', 'alt+shift+Right')
            wait_for(blue, blue, 'alt+shift+Right did not move the tab into the right pane')
            self.assertEqual(pixel(.505, 4 / 500), background, 'the emptied pane outlived the move')

    def test_a_worminal_input_link_types_its_text_once(self):
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

    def test_a_browser_tab_moves_into_a_split_keeping_its_page_and_the_terminal_painting(self):
        from Xlib import X, display, error
        red, green, white = (200, 30, 30), (30, 160, 30), (255, 255, 255)
        with isolated_display() as env, ExitStack() as cleanup:
            root = CHECKS / f'browser-split-{os.getpid()}'
            shutil.rmtree(root, ignore_errors=True)
            root.mkdir(parents=True)
            self.addCleanup(shutil.rmtree, root, ignore_errors=True)
            # The title is random per load, so a reloaded page (a lost DOM) shows a different one.
            page = root / 'page.html'
            page.write_text('<!doctype html><title>t</title><body style="margin:0;background:#fff">'
                            '<script>document.title="doc-" + Math.floor(Math.random() * 1e9)</script>')
            # A terminal that changes color every quarter second shows whether its frames still arrive:
            # a hidden tab would freeze on whichever color it last painted.
            flip = ("while :; do printf '\\033[48;2;200;30;30m\\033[2J'; sleep .25; "
                    "printf '\\033[48;2;30;160;30m\\033[2J'; sleep .25; done")
            env.update(GDK_BACKEND='x11', XDG_CONFIG_HOME=str(root / 'config'))
            env.pop('WAYLAND_DISPLAY', None)
            app = subprocess.Popen([str(runtime_binary('worminal-rust')), '-T', 'Rust browser split', '-o', '-',
                                    '-e', '/bin/sh', '-c', flip],
                                   env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            self.addCleanup(lambda: self.stop(app))
            wid = window(env, app)
            xdo = lambda *args: subprocess.run(['xdotool', *args], env=env, check=True)
            api = lambda *args: subprocess.check_output([str(runtime_binary('worminald')), 'api', *args],
                                                        env=env, text=True)
            title = lambda: subprocess.check_output(['xdotool', 'getwindowname', wid], env=env, text=True).strip()
            server = display.Display(env['DISPLAY'])
            cleanup.callback(server.close)
            window_object = server.create_resource_object('window', int(wid))
            xdo('windowfocus', '--sync', wid)
            xdo('windowsize', wid, '900', '500')
            time.sleep(1.5)

            def pixel(fx, fy):
                try:
                    data = window_object.get_image(int(900 * fx), int(500 * fy), 1, 1, X.ZPixmap, 0xffffffff).data
                except error.BadMatch:
                    return None
                return tuple(data[:3][::-1])

            def wait_for(condition, message):
                deadline = time.monotonic() + 10
                while not condition():
                    self.assertIsNone(app.poll())
                    self.assertLess(time.monotonic(), deadline, message)
                    time.sleep(.05)
            tabs = lambda: json.loads(api('list'))['tabs']
            api('open', '--browser', page.as_uri())
            wait_for(lambda: len(tabs()) == 2 and tabs()[1]['kind'] == 'browser', 'no browser tab appeared')
            xdo('key', '--clearmodifiers', 'alt+2')
            wait_for(lambda: pixel(.5, .75) == white, 'the browser page was not shown')
            wait_for(lambda: title().startswith('doc-'), 'the page never reported its title')
            loaded = title()
            # Alt+Shift+Right moves the browser tab into a pane of its own, beside the terminal.
            xdo('key', '--clearmodifiers', 'alt+shift+Right')
            wait_for(lambda: pixel(.75, .75) == white and pixel(.25, .75) in (red, green),
                     'the browser tab did not split off beside the terminal')
            # The page moved with its widget: it was not reloaded.
            time.sleep(2)
            self.assertEqual(title(), loaded, 'moving the browser tab reloaded its page')
            # The terminal beside the browser still paints: both of its colors keep arriving.
            seen = set()
            deadline = time.monotonic() + 4
            while time.monotonic() < deadline and not {red, green} <= seen:
                seen.add(pixel(.25, .75))
                time.sleep(.05)
            self.assertTrue({red, green} <= seen,
                            f'the terminal beside the browser stopped painting: saw {seen}')
            self.assertEqual(pixel(.75, .75), white)
            # Right after the split the terminal's own resize keeps it shown, which hides a fault: selecting the
            # browser pane again later must not hide the terminal, as an exclusive Focus on the browser would.
            xdo('key', '--clearmodifiers', 'ctrl+shift+Left')
            time.sleep(1)
            xdo('key', '--clearmodifiers', 'ctrl+shift+Right')
            wait_for(lambda: title() == loaded, 'the browser pane was not selected again')
            time.sleep(1)
            seen = set()
            deadline = time.monotonic() + 4
            while time.monotonic() < deadline and not {red, green} <= seen:
                seen.add(pixel(.25, .75))
                time.sleep(.05)
            self.assertTrue({red, green} <= seen,
                            f'selecting the browser pane hid the terminal beside it: saw {seen}')

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

            def panel_open():
                try:
                    row = window_object.get_image(0, 250, 900, 1, X.ZPixmap, 0xffffffff).data
                except error.BadMatch:
                    return False
                return any(len(set(row[x * 4:x * 4 + 3])) == 1 and 70 <= row[x * 4] <= 95 for x in range(60, 600))

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
            app = subprocess.Popen([str(runtime_binary('worminal-rust')), '-T', 'Rust panel', '-o', '-',
                                    '-e', '/bin/sh', '-c', 'while :; do sleep 1; done'],
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

            def panel_edge():
                # The panel draws its right edge in a faint gray (about 30% toward the foreground) that this
                # test's terminals never use; the terminal background is (24, 24, 24).
                try:
                    row = window_object.get_image(0, 250, 900, 1, X.ZPixmap, 0xffffffff).data
                except error.BadMatch:
                    return False
                edge = lambda x: len(set(row[x * 4:x * 4 + 3])) == 1 and 70 <= row[x * 4] <= 95
                return any(edge(x) for x in range(60, 600))

            def wait_for(condition, message):
                deadline = time.monotonic() + 8
                while not condition():
                    self.assertIsNone(app.poll())
                    self.assertLess(time.monotonic(), deadline, message)
                    time.sleep(.05)
            tabs = lambda: json.loads(api('list'))['tabs']
            wait_for(lambda: tabs()[0].get('directory') == str(start), 'the tab never reported its directory')
            self.assertFalse(panel_edge(), 'the panel is open before it was asked for')
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

    def test_tab_strips_rewrap_when_splits_change_their_width(self):
        from Xlib import X, display
        red, blue = (200, 30, 30), (20, 40, 210)
        fill = lambda color: ("while :; do printf '\\033[48;2;%d;%d;%dm\\033[2J'; sleep .2; done" % color)
        # Labels are the directory basename; at 40 cells two fit a 900px row but only one fits half of it.
        directory = CHECKS / ('w' * 40)
        directory.mkdir(exist_ok=True)
        with isolated_display() as env, ExitStack() as cleanup:
            script = CHECKS / 'rewrap-shell.sh'
            script.write_text(f'#!/bin/sh\ncd {shlex.quote(str(directory))}\n{fill(blue)}\n')
            script.chmod(0o755)
            env.update(GDK_BACKEND='x11', SHELL=str(script))
            env.pop('WAYLAND_DISPLAY', None)
            app = subprocess.Popen([str(runtime_binary("worminal-rust")), '-T', 'Rust rewrap', '-o', '-',
                                    '-e', '/bin/sh', '-c', fill(red)], cwd=directory,
                                   env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            self.addCleanup(lambda: self.stop(app))
            wid = window(env, app)
            xdo = lambda *args: subprocess.run(['xdotool', *args], env=env, check=True)
            api = lambda *args: subprocess.check_output([str(runtime_binary('worminald')), 'api', *args],
                                                        env=env, text=True)
            server = display.Display(env['DISPLAY'])
            cleanup.callback(server.close)
            root = server.create_resource_object('window', int(wid))
            self.assertEqual(fix_size(root, xdo, wid, 900, 500), (900, 500))

            def strip_height(x):
                # The strip ends where the terminal's own fill begins.
                data = root.get_image(x, 0, 1, 500, X.ZPixmap, 0xffffffff).data
                column = [tuple(data[4 * y:4 * y + 3][::-1]) for y in range(500)]
                return next((y for y, color in enumerate(column) if color in (red, blue)), None)

            def fail(message):
                from PIL import Image
                data = root.get_image(0, 0, 900, 500, X.ZPixmap, 0xffffffff).data
                Image.frombuffer('RGB', (900, 500), data, 'raw', 'BGRX', 0, 1).save(CHECKS / 'rust-app-rewrap.png')
                self.fail(message)

            def wait_height(x, expected, message):
                deadline = time.monotonic() + 8
                while strip_height(x) != expected:
                    self.assertIsNone(app.poll())
                    if time.monotonic() >= deadline:
                        fail(f'{message}: {strip_height(x)} != {expected}')
                    time.sleep(.05)

            def settle(x):
                # Strip rows are whatever the font makes them, so heights are compared once they stop changing.
                last, since, deadline = None, time.monotonic(), time.monotonic() + 8
                while time.monotonic() - since < .5:
                    self.assertLess(time.monotonic(), deadline, 'the strip height did not settle')
                    if strip_height(x) != last:
                        last, since = strip_height(x), time.monotonic()
                    time.sleep(.05)
                return last

            one = settle(225)
            self.assertIsNotNone(one, 'first tab did not paint')
            xdo('windowfocus', '--sync', wid)
            for count in (2, 3, 4):
                xdo('key', '--clearmodifiers', 'ctrl+shift+t')
                deadline = time.monotonic() + 5
                while len(json.loads(api('list'))['tabs']) != count:
                    self.assertLess(time.monotonic(), deadline, f'tab {count} did not open')
                    time.sleep(.05)
            # New labels follow each shell's directory once the daemon reports it.
            deadline = time.monotonic() + 8
            while strip_height(225) in (one, None):
                if time.monotonic() >= deadline:
                    fail(f'four tabs did not wrap onto a second row: {strip_height(225)} at {window_size(root)}')
                time.sleep(.05)
            row = settle(225) - one
            # Splitting off the selected tab leaves three in a half-width strip: one per row.
            xdo('key', '--clearmodifiers', 'alt+shift+Right')
            wait_height(225, one + 2 * row, 'the narrowed strip did not wrap onto three rows')
            wait_height(675, one, 'the new pane strip is not one row')
            # Gathering the tabs widens the strip again.
            xdo('key', '--clearmodifiers', 'ctrl+shift+x')
            wait_height(225, one + row, 'the widened strip did not unwrap to two rows')
            wait_height(675, one + row, 'the widened strip did not span the window')

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
    suite.addTests(unittest.defaultTestLoader.loadTestsFromTestCase(FileDropTest))
    suite.addTests(unittest.defaultTestLoader.loadTestsFromTestCase(MasterDetachTest))
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
