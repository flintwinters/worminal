"""A window reopens with the split layout, divider, size and position it last had (docs/layout-restore.md)."""
from runtime import (CHECKS, runtime_binary, window, stop_application, copy_runtime, module_path,
                     RUST_FRONTEND_MODULE)
from preview import profile, service
from workspace_protocol import send, wait_kind, service_pid
from x11 import isolated_display

from contextlib import ExitStack
import json
import os
import shutil
import signal
import socket
import subprocess
import tempfile
import time
from pathlib import Path
import unittest
import uuid

RED, BLUE = (200, 30, 30), (20, 40, 210)


def fill(color, title):
    # Each terminal repaints its whole screen continuously, so a resize never leaves erased background.
    return ("while :; do printf '\\033]2;%s\\007\\033[48;2;%d;%d;%dm\\033[2J'; sleep .2; done"
            % ((title,) + color))


class LayoutRestoreTest(unittest.TestCase):
    def launch(self, env, *args):
        app = subprocess.Popen([str(runtime_binary('worminal-rust')), *args], env=env,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.exit.callback(lambda: stop_application(app) if app.poll() is None else app.communicate(timeout=5))
        return app

    def private(self, env, config=None):
        """A private state and config home, and a shell that paints its tab blue."""
        root = CHECKS / f'layout-{os.getpid()}-{uuid.uuid4().hex[:8]}'
        root.mkdir(parents=True)
        self.addCleanup(shutil.rmtree, root, ignore_errors=True)
        script = root / 'blue-shell.sh'
        script.write_text(f'#!/bin/sh\n{fill(BLUE, "blue")}\n')
        script.chmod(0o755)
        (root / 'config/worminal').mkdir(parents=True)
        if config:
            (root / 'config/worminal/worminal.yaml').write_text(config)
        env.update(GDK_BACKEND='x11', SHELL=str(script), XDG_CONFIG_HOME=str(root / 'config'),
                   XDG_STATE_HOME=str(root / 'state'))
        env.pop('WAYLAND_DISPLAY', None)
        return root / 'state/worminal'

    def saved(self, state):
        files = sorted(state.glob('layout-v*.json')) if state.exists() else []
        return json.loads(files[0].read_text()) if files else None

    def wait_saved(self, state, ready, message):
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            try:
                saved = self.saved(state)
            except json.JSONDecodeError:
                saved = None
            if saved and ready(saved):
                return saved
            time.sleep(.05)
        self.fail(f'{message}: {self.saved(state)}')

    def test_a_split_comes_back_after_a_frontend_crash_and_after_a_daemon_restart(self):
        from Xlib import X, display, error
        with isolated_display() as env, ExitStack() as stack:
            self.exit = stack  # closes before the display goes away
            state = self.private(env)
            xdo = lambda *args: subprocess.run(['xdotool', *args], env=env, check=True)
            api = lambda *args: subprocess.check_output([str(runtime_binary('worminald')), 'api', *args],
                                                        env=env, text=True)
            server = display.Display(env['DISPLAY'])
            self.exit.callback(server.close)

            def view(app):
                wid = window(env, app)
                return wid, server.create_resource_object('window', int(wid))

            def size(root):
                geometry = root.get_geometry()
                return geometry.width, geometry.height

            def pixel(root, fx, fy):
                width, height = size(root)
                try:
                    data = root.get_image(int(width * fx), int(height * fy), 1, 1, X.ZPixmap, 0xffffffff).data
                except error.BadMatch:
                    return None
                return tuple(data[:3][::-1])

            def wait_colors(app, root, left, right, message):
                deadline = time.monotonic() + 8
                while (pixel(root, .15, .75), pixel(root, .65, .75)) != (left, right):
                    if app.poll() is not None:
                        self.fail('the application exited: ' + app.stderr.read().decode(errors='replace'))
                    self.assertLess(time.monotonic(), deadline,
                                    f'{message}: {pixel(root, .15, .75)} {pixel(root, .65, .75)}')
                    time.sleep(.05)

            def wait_tabs(count):
                deadline = time.monotonic() + 8
                while len(json.loads(api('list'))['tabs']) != count:
                    self.assertLess(time.monotonic(), deadline, f'daemon did not reach {count} tabs')
                    time.sleep(.05)

            def divider(root):
                """The columns of one row, in the middle band, that are not the blue fill: borders and red pane."""
                width, height = size(root)
                data = root.get_image(0, int(height * .75), width, 1, X.ZPixmap, 0xffffffff).data
                return [x for x in range(int(width * .1), int(width * .9))
                        if tuple(data[x * 4:x * 4 + 3][::-1]) != BLUE]

            app = self.launch(env, '-T', 'Layout one', '-o', '-', '-e', '/bin/sh', '-c', fill(RED, 'red'))
            wid, root = view(app)
            xdo('windowfocus', '--sync', wid)
            # Settle on the grid-fitted size first, then take the size and place the layout is to keep.
            time.sleep(1)
            xdo('windowsize', wid, '900', '500')
            xdo('windowmove', wid, '60', '40')
            xdo('key', '--clearmodifiers', 'ctrl+shift+t')
            wait_tabs(2)
            # The new blue tab is the current one; Alt+Shift+Right splits it off to the right of the red tab.
            xdo('key', '--clearmodifiers', 'alt+shift+Right')
            wait_colors(app, root, RED, BLUE, 'the split did not happen')
            # Drag the divider to about 30%: the layout keeps where it sits, not just that it exists.
            xdo('mousemove', '--window', wid, '450', '300', 'mousedown', '1')
            for step in range(1, 11):
                xdo('mousemove', '--window', wid, str(450 - 180 * step // 10), '300')
                time.sleep(.02)
            xdo('mouseup', '1')
            saved = self.wait_saved(
                state, lambda saved: saved['windows'][0]['tree'].get('ratio', 500) < 400
                and saved['windows'][0]['geometry'][2:] == [900, 500],
                'the layout with a dragged divider was not saved')
            window_saved = saved['windows'][0]
            self.assertEqual([window_saved['tree']['first']['tabs'], window_saved['tree']['second']['tabs']],
                             [[0], [1]])
            self.assertEqual(window_saved['geometry'][:2], [60, 40])
            ratio = window_saved['tree']['ratio']
            self.assertTrue(250 <= ratio <= 350, ratio)
            # The window had no real window manager to place it, so only the saved size is checked below.

            # 1. The frontend crashes; the daemon and both shells survive. Reopening restores the layout.
            app.kill()
            app.communicate(timeout=5)
            app = self.launch(env, '--attach', '-T', 'Layout two')
            wid, root = view(app)
            wait_colors(app, root, RED, BLUE, 'the split did not return after a frontend crash')
            self.assertEqual(size(root), (900, 500), 'the window did not reopen at its saved size')
            # The left pane is the saved ~30%: x = 25% is still red, x = 40% is already blue.
            self.assertEqual((pixel(root, .25, .75), pixel(root, .4, .75)), (RED, BLUE),
                             'the divider did not return to its saved place')

            # 2. The daemon goes too. Its tabs recover as fresh shells (both now blue), and the layout still comes back.
            app.kill()
            app.communicate(timeout=5)
            with socket.socket(socket.AF_UNIX) as observer:
                observer.settimeout(3)
                observer.connect(f"\0worminal-workspace-v2-{os.getuid()}-{env['WORMINAL_SHARED_SOCKET_SCOPE']}")
                send(observer, 19, version=2)
                pid = service_pid(observer)
            os.kill(pid, signal.SIGKILL)
            deadline = time.monotonic() + 5
            while os.path.exists(f'/proc/{pid}'):
                self.assertLess(time.monotonic(), deadline, 'the daemon did not die')
                time.sleep(.05)
            app = self.launch(env, '--attach', '-T', 'Layout three')
            wid, root = view(app)
            wait_tabs(2)
            deadline = time.monotonic() + 8
            while True:
                columns = divider(root)
                # Two panes of the same blue are told apart by their borders, near 30% of the width.
                if columns and 0.25 * 900 < columns[0] < 0.35 * 900:
                    break
                self.assertLess(time.monotonic(), deadline,
                                f'the recovered tabs did not return to their panes: {columns[:6]}')
                time.sleep(.1)
            self.assertEqual(size(root), (900, 500))

    def test_a_new_window_opens_with_the_split_of_the_window_it_opened_from(self):
        """Windows are views of one set of tabs: Ctrl+Shift+N once opened a single pane beside a split window,
        because only the first window of a process takes a layout from the file."""
        from Xlib import X, display
        with isolated_display() as env, ExitStack() as stack:
            self.exit = stack
            self.private(env)
            xdo = lambda *args: subprocess.run(['xdotool', *args], env=env, check=True)
            server = display.Display(env['DISPLAY'])
            self.exit.callback(server.close)
            app = self.launch(env, '-T', 'Split source', '-e', '/bin/sh', '-c', fill(RED, 'red'))
            wid = window(env, app)
            xdo('windowfocus', '--sync', wid)
            time.sleep(1)
            xdo('key', '--clearmodifiers', 'ctrl+shift+t')
            time.sleep(.5)
            xdo('key', '--clearmodifiers', 'alt+shift+Right')
            time.sleep(.5)
            xdo('key', '--clearmodifiers', 'ctrl+shift+n')
            deadline = time.monotonic() + 8
            while True:
                found = subprocess.run(['xdotool', 'search', '--onlyvisible', '--pid', str(app.pid)],
                                       env=env, capture_output=True, text=True).stdout.split()
                if len(found) == 2:
                    break
                self.assertLess(time.monotonic(), deadline, 'the second window did not open')
                time.sleep(.05)
            root = server.create_resource_object('window', int(next(w for w in found if w != wid)))
            # The new shell joins the first pane, so both panes are blue; pane borders mark the split.
            deadline = time.monotonic() + 8
            while True:
                geometry = root.get_geometry()
                row = root.get_image(0, int(geometry.height * .75), geometry.width, 1, X.ZPixmap,
                                     0xffffffff).data
                color = lambda x: tuple(row[x * 4:x * 4 + 3][::-1])
                columns = [x for x in range(int(geometry.width * .3), int(geometry.width * .7))
                           if color(x) != BLUE]
                # Both sides painted blue first: an unpainted window has no blue at all.
                if columns and color(int(geometry.width * .2)) == color(int(geometry.width * .8)) == BLUE:
                    break
                self.assertLess(time.monotonic(), deadline, 'the new window opened without the split')
                time.sleep(.1)

    def test_restoring_can_be_turned_off_and_a_window_can_be_closed_without_its_layout(self):
        from Xlib import X, display, error
        with isolated_display() as env, ExitStack() as stack:
            self.exit = stack  # closes before the display goes away
            state = self.private(env)
            xdo = lambda *args: subprocess.run(['xdotool', *args], env=env, check=True)
            server = display.Display(env['DISPLAY'])
            self.exit.callback(server.close)

            def pixels(app):
                wid = window(env, app)
                root = server.create_resource_object('window', int(wid))

                def get(fx):
                    geometry = root.get_geometry()  # the window fits itself to its first terminal's grid
                    data = root.get_image(int(geometry.width * fx), int(geometry.height * .75), 1, 1,
                                          X.ZPixmap, 0xffffffff).data
                    return tuple(data[:3][::-1])
                return wid, get

            api = lambda *args: subprocess.check_output([str(runtime_binary('worminald')), 'api', *args],
                                                        env=env, text=True)

            def split(app):
                """Open a second (blue) tab and split it off to the right of the first (red) one."""
                wid, get = pixels(app)
                xdo('windowfocus', '--sync', wid)
                time.sleep(1)
                xdo('key', '--clearmodifiers', 'ctrl+shift+t')
                deadline = time.monotonic() + 8
                while len(json.loads(api('list'))['tabs']) != 2:
                    self.assertLess(time.monotonic(), deadline, 'second tab did not open')
                    time.sleep(.05)
                xdo('key', '--clearmodifiers', 'alt+shift+Right')
                deadline = time.monotonic() + 8
                while (get(.15), get(.65)) != (RED, BLUE):
                    self.assertLess(time.monotonic(), deadline, f'could not split: {get(.15)} {get(.65)}')
                    time.sleep(.05)
                return wid, get

            # Ctrl+Shift+Q closes the window and forgets its layout.
            app = self.launch(env, '-T', 'Layout one', '-o', '-', '-e', '/bin/sh', '-c', fill(RED, 'red'))
            wid, get = split(app)
            self.wait_saved(state, lambda saved: saved['windows'][0]['tree'].get('split'), 'no layout was saved')
            xdo('key', '--clearmodifiers', 'ctrl+shift+q')
            self.assertEqual(app.wait(timeout=8), 0)
            self.assertIsNone(self.saved(state), 'the closed window left its layout behind')
            app = self.launch(env, '--attach', '-T', 'Layout two')
            wid, get = pixels(app)
            time.sleep(1.5)
            self.assertEqual(get(.15), get(.65), 'a forgotten layout came back')

            # A plain close keeps it: the same split returns.
            xdo('windowfocus', '--sync', wid)
            xdo('key', '--clearmodifiers', 'alt+shift+Right')
            deadline = time.monotonic() + 8
            while get(.15) == get(.65):
                self.assertLess(time.monotonic(), deadline, 'could not split again')
                time.sleep(.1)
            self.wait_saved(state, lambda saved: saved['windows'][0]['tree'].get('split'), 'the split was not saved')
            from x11 import close_window
            close_window(env, subprocess.check_output(['xdotool', 'getwindowname', wid], env=env, text=True).strip())
            self.assertEqual(app.wait(timeout=8), 0)
            app = self.launch(env, '--attach', '-T', 'Layout three')
            wid, get = pixels(app)
            deadline = time.monotonic() + 8
            while get(.15) == get(.65):
                self.assertLess(time.monotonic(), deadline, 'a closed window did not reopen split')
                time.sleep(.1)
            app.kill()
            app.communicate(timeout=5)

    def test_restore_layout_false_starts_every_window_fresh(self):
        from Xlib import X, display
        with isolated_display() as env, ExitStack() as stack:
            self.exit = stack  # closes before the display goes away
            state = self.private(env, 'worminal:\n  restore_layout: false\n')
            xdo = lambda *args: subprocess.run(['xdotool', *args], env=env, check=True)
            server = display.Display(env['DISPLAY'])
            self.exit.callback(server.close)
            app = self.launch(env, '-T', 'Layout one', '-o', '-', '-e', '/bin/sh', '-c', fill(RED, 'red'))
            wid = window(env, app)
            root = server.create_resource_object('window', int(wid))
            xdo('windowfocus', '--sync', wid)
            time.sleep(1)
            xdo('key', '--clearmodifiers', 'ctrl+shift+t')
            time.sleep(1)
            xdo('key', '--clearmodifiers', 'alt+shift+Right')
            time.sleep(1.5)
            width, height = root.get_geometry().width, root.get_geometry().height
            pixel = lambda fx: tuple(root.get_image(int(width * fx), int(height * .75), 1, 1,
                                                    X.ZPixmap, 0xffffffff).data[:3][::-1])
            self.assertNotEqual(pixel(.15), pixel(.65), 'the split did not happen')
            time.sleep(1)
            self.assertIsNone(self.saved(state), 'a layout was saved although restore_layout is false')

    def test_a_window_update_keeps_the_split_and_the_new_windows_keep_saving(self):
        """The update swaps each window for one in a new process; the split must survive the swap."""
        from Xlib import X, display
        with (isolated_display() as shown, profile(shown) as env, service(env),
              tempfile.TemporaryDirectory(prefix='layout-update-', dir=CHECKS) as directory,
              ExitStack() as stack):
            self.exit = stack
            base = Path(directory)
            executable = copy_runtime(base, browser=True)
            script = base / 'blue-shell.sh'
            script.write_text(f'#!/bin/sh\n{fill(BLUE, "blue")}\n')
            script.chmod(0o755)
            env = {**env, 'GDK_BACKEND': 'x11', 'GIO_USE_VFS': 'local', 'SHELL': str(script)}
            env.pop('DBUS_SESSION_BUS_ADDRESS', None)
            env.pop('WAYLAND_DISPLAY', None)
            state = Path(env['XDG_STATE_HOME']) / 'worminal'
            xdo = lambda *args: subprocess.run(['xdotool', *args], env=env, check=True)
            log = stack.enter_context((base / 'owner.log').open('wb'))
            owner = subprocess.Popen([str(executable), '-c', 'LayoutFixture', '-T', 'Layout update', '-e', '/bin/sh', '-c', fill(RED, 'red')],
                                     env=env, stdout=log, stderr=log)
            stack.callback(lambda: owner.poll() is None and owner.terminate())
            server = display.Display(env['DISPLAY'])
            stack.callback(server.close)

            def windows():
                found = subprocess.run(['xdotool', 'search', '--onlyvisible', '--class', 'LayoutFixture'],
                                       env=env, capture_output=True, text=True)
                return found.stdout.split() if found.returncode == 0 else []

            def pid(wid):
                return subprocess.run(['xdotool', 'getwindowpid', wid], env=env, capture_output=True,
                                      text=True).stdout.strip()

            def colors(wid):
                root = server.create_resource_object('window', int(wid))
                geometry = root.get_geometry()
                get = lambda fx: tuple(root.get_image(int(geometry.width * fx), int(geometry.height * .75), 1, 1,
                                                      X.ZPixmap, 0xffffffff).data[:3][::-1])
                return get(.15), get(.65)

            def wait(check, message):
                deadline = time.monotonic() + 15
                while not check():
                    self.assertLess(time.monotonic(), deadline, message + (base / 'owner.log').read_text(errors='replace'))
                    time.sleep(.1)

            wait(windows, 'no window: ')
            wid = windows()[0]
            donor = pid(wid)
            xdo('windowfocus', '--sync', wid)
            time.sleep(1)
            xdo('key', '--clearmodifiers', 'ctrl+shift+t')
            time.sleep(1)
            xdo('key', '--clearmodifiers', 'alt+shift+Right')
            wait(lambda: colors(wid) == (RED, BLUE), 'the split did not happen: ')
            wait(lambda: (self.saved(state) or {}).get('windows', [{}])[0].get('tree', {}).get('split'),
                 'the split was not saved: ')

            # Replace the frontend module, as an installed update does.
            module = module_path(RUST_FRONTEND_MODULE, prefix=base)
            pending = module.with_suffix('.pending')
            shutil.copy2(module, pending)
            pending.replace(module)
            wait(lambda: windows() and pid(windows()[0]) != donor, 'the owner did not replace itself: ')
            wid = windows()[0]
            wait(lambda: colors(wid) == (RED, BLUE), 'the split did not survive the window update: ')
            # The new process owns the window now, and it saves what it shows: a change the donor is no longer there to write.
            xdo('windowsize', wid, '810', '470')
            wait(lambda: self.saved(state)['windows'][0]['geometry'][2:] == [810, 470],
                 'the replacement window does not save its layout: ')
            self.assertTrue(self.saved(state)['windows'][0]['tree'].get('split'))

    def test_a_daemon_restart_right_after_a_window_update_keeps_the_split(self):
        """A real update restarts the daemon as the new window opens, before that window has saved anything: the
        window must still rebuild its layout when the daemon is back, not stay one pane."""
        from Xlib import X, display
        with (isolated_display() as shown, profile(shown) as env, service(env),
              tempfile.TemporaryDirectory(prefix='layout-update-', dir=CHECKS) as directory,
              ExitStack() as stack):
            self.exit = stack
            base = Path(directory)
            executable = copy_runtime(base, browser=True)
            script = base / 'blue-shell.sh'
            script.write_text(f'#!/bin/sh\n{fill(BLUE, "blue")}\n')
            script.chmod(0o755)
            env = {**env, 'GDK_BACKEND': 'x11', 'GIO_USE_VFS': 'local', 'SHELL': str(script)}
            env.pop('DBUS_SESSION_BUS_ADDRESS', None)
            env.pop('WAYLAND_DISPLAY', None)
            state = Path(env['XDG_STATE_HOME']) / 'worminal'
            xdo = lambda *args: subprocess.run(['xdotool', *args], env=env, check=True)
            log = stack.enter_context((base / 'owner.log').open('wb'))
            owner = subprocess.Popen([str(executable), '-c', 'LayoutFixture', '-T', 'Layout update', '-e', '/bin/sh', '-c', fill(RED, 'red')],
                                     env=env, stdout=log, stderr=log)
            stack.callback(lambda: owner.poll() is None and owner.terminate())

            def windows():
                found = subprocess.run(['xdotool', 'search', '--onlyvisible', '--class', 'LayoutFixture'],
                                       env=env, capture_output=True, text=True)
                return found.stdout.split() if found.returncode == 0 else []

            def pid(wid):
                return subprocess.run(['xdotool', 'getwindowpid', wid], env=env, capture_output=True,
                                      text=True).stdout.strip()

            def wait(check, message, seconds=15):
                deadline = time.monotonic() + seconds
                while not check():
                    self.assertLess(time.monotonic(), deadline, message + (base / 'owner.log').read_text(errors='replace'))
                    time.sleep(.05)

            def split():
                return (self.saved(state) or {}).get('windows', [{}])[0].get('tree', {}).get('split')

            wait(windows, 'no window: ')
            wid = windows()[0]
            donor = pid(wid)
            xdo('windowfocus', '--sync', wid)
            time.sleep(1)
            xdo('key', '--clearmodifiers', 'ctrl+shift+t')
            time.sleep(1)
            xdo('key', '--clearmodifiers', 'alt+shift+Right')
            wait(split, 'the split was not saved: ')
            with socket.socket(socket.AF_UNIX) as observer:
                observer.settimeout(3)
                observer.connect(f"\0worminal-workspace-v2-{os.getuid()}-{env['WORMINAL_SHARED_SOCKET_SCOPE']}")
                send(observer, 19, version=2)
                daemon = service_pid(observer)
            module = module_path(RUST_FRONTEND_MODULE, prefix=base)
            pending = module.with_suffix('.pending')
            shutil.copy2(module, pending)
            pending.replace(module)
            wait(lambda: windows() and pid(windows()[0]) != donor, 'the owner did not replace itself: ')
            os.kill(daemon, signal.SIGKILL)
            # Give the new window time to reconnect and to save (the settle time is half a second).
            time.sleep(4)
            self.assertTrue(split(), 'the window came back from the daemon restart as one pane and saved that')

