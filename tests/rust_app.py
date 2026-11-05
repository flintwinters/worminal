"""Exercise the actual Rust application with private PTYs, windows, tabs and reattachment."""
from runtime import CHECKS

from runtime import runtime_binary

from contextlib import ExitStack
import os
import fcntl
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
from test_keys import NavigationKeysTest, read_bytes, read_through
from workspace_protocol import send, wait_kind, catalog_records, service_pid
from x11 import isolated_display, close_window
from preview import profile, service, wayland_display
from test_window_state import private_bus, WindowStateTest
from test_window_activation import WindowActivationTests
from runtime import copy_runtime, window, stop_application
from rust_module import RustModuleTest
from test_runtime import RuntimeCleanupTest

ROOT = Path(__file__).resolve().parent.parent


class RustApplicationTest(unittest.TestCase):
    def test_rust_view_uses_historical_c_v2_service_without_restarting_its_shell(self):
        with isolated_display() as display, profile(display) as env:
            env.update(GDK_BACKEND='x11', WORMINAL_TEST_PREVIEW_DAEMON=str(ROOT / 'build/worminald-c-v2'))
            env.pop('WAYLAND_DISPLAY', None)
            with service(env):
                app = subprocess.Popen([str(runtime_binary("worminal-rust")), '-T', 'C service attachment', '-o', '-',
                    '-e', '/bin/sh', '-c', "stty raw -echo; printf 'C:%s\\n' $$; exec cat"], env=env,
                    stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                self.addCleanup(lambda: self.stop(app))
                shell = int(read_through(app.stdout, b'\n')[2:].strip())
                tty = os.readlink(f'/proc/{shell}/fd/0')
                wid = window(env, app)
                subprocess.run(['xdotool', 'windowfocus', '--sync', wid, 'type', '--clearmodifiers', 'peer'],
                               env=env, check=True)
                self.assertEqual(read_bytes(app.stdout, 4), b'peer')
                close_window(env, 'C service attachment')
                self.assertEqual(app.wait(timeout=5), 0)
                self.assertEqual(os.readlink(f'/proc/{shell}/fd/0'), tty)

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

    @staticmethod
    def stop(process):
        stop_application(process)


def main():
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(RustApplicationTest)
    suite.addTests(unittest.defaultTestLoader.loadTestsFromTestCase(WindowStateTest))
    suite.addTests(unittest.defaultTestLoader.loadTestsFromTestCase(WindowActivationTests))
    suite.addTests(unittest.defaultTestLoader.loadTestsFromTestCase(RustModuleTest))
    suite.addTests(unittest.defaultTestLoader.loadTestsFromTestCase(RuntimeCleanupTest))
    for name in ('test_navigation_keys', 'test_alternate_scroll_sends_cursor_keys',
                 'test_clipboard_paste_bytes', 'test_ctrl_end_in_micro', 'test_master_navigation_keys',
                 'test_master_alternate_scroll_sends_cursor_keys', 'test_master_clipboard_paste_bytes'):
        suite.addTest(NavigationKeysTest(name))
    with (private_bus(os.environ) as env,
          patch.dict(os.environ, {'DBUS_SESSION_BUS_ADDRESS': env['DBUS_SESSION_BUS_ADDRESS']})):
        return 0 if unittest.TextTestRunner(verbosity=1).run(suite).wasSuccessful() else 1


if __name__ == '__main__':
    raise SystemExit(main())
