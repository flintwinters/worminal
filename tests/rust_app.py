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

            def size():
                geometry = root.get_geometry()
                return geometry.width, geometry.height

            def settle():
                # The first terminal fits the window to its character grid, then the test fixes the size.
                last, since = None, time.monotonic()
                while time.monotonic() - since < .5:
                    if size() != last:
                        last, since = size(), time.monotonic()
                    time.sleep(.02)
                return last
            settle()
            xdo('windowsize', wid, '900', '500')
            self.assertEqual(settle(), (900, 500))

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
            xdo('mouseup', '1')
            # The anchor stays left; the dropped tab takes the right half and focus.
            wait_for(blue, red, 'dropping a tab on the right edge did not split the window')
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
            # Dragging the right pane's tab onto the left strip puts it back there and ends the split.
            xdo('mousemove', '--window', wid, '490', '8', 'mousedown', '1')
            for step in range(1, 11):
                xdo('mousemove', '--window', wid, str(490 - 400 * step // 10), '8')
                time.sleep(.02)
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
            self.assertEqual(len(json.loads(api('list'))['tabs']), 1)

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
            # Ctrl+Shift+P opens the panel; Enter opens the selected bookmark in a new tab and closes it.
            xdo('key', '--clearmodifiers', 'ctrl+shift+p')
            wait_for(panel_edge, 'ctrl+shift+p did not open the panel')
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
            xdo('key', '--clearmodifiers', 'ctrl+shift+p')
            wait_for(panel_edge, 'ctrl+shift+p did not reopen the panel')
            xdo('key', '--clearmodifiers', 'Escape')
            wait_for(lambda: not panel_edge(), 'Escape did not close the panel')
            xdo('key', '--clearmodifiers', 'ctrl+shift+p')
            wait_for(panel_edge, 'ctrl+shift+p did not reopen the panel')
            xdo('key', '--clearmodifiers', 'Delete')
            wait_for(lambda: str(other) not in saved.read_text(), 'Delete did not remove the bookmark')
            self.assertEqual(len(tabs()), 2)

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
