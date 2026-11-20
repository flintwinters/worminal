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
            env.update(GDK_BACKEND='x11', SHELL=str(script))
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
            xdo('mousemove', '--window', wid, '12', '8', 'mousedown', '1')
            for step in range(1, 11):
                xdo('mousemove', '--window', wid, str(12 + 876 * step // 10), str(8 + 250 * step // 10))
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
            xdo('mousemove', '--window', wid, '462', '8', 'mousedown', '1')
            for step in range(1, 11):
                xdo('mousemove', '--window', wid, str(462 - 400 * step // 10), '8')
                time.sleep(.02)
            xdo('mouseup', '1')
            wait_for(red, red, 'dropping a tab on the other strip did not merge the panes')
            self.assertEqual(pixel(.505, 4 / 500), background, 'the right strip outlived the split')
            # Split again, then Ctrl+Shift+X gathers the tabs.
            xdo('mousemove', '--window', wid, '12', '8', 'mousedown', '1')
            for step in range(1, 11):
                xdo('mousemove', '--window', wid, str(12 + 876 * step // 10), str(8 + 250 * step // 10))
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
