"""Exercise lazy Rust WebKit modules through the actual native application on private displays."""
from contextlib import contextmanager
import json
import hashlib
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

from preview import broker_check, Fixture, fixture, profile, service, wayland_display
from x11 import isolated_display, close_window
from test_window_state import private_bus
from rust_runtime import copy_runtime, RUST_BROWSER_MODULE

ROOT = Path(__file__).resolve().parent.parent
APP = str(ROOT / 'worminal-rust')


@contextmanager
def application(env, *args, executable=APP):
    with (ROOT / '.checks/rust-browser-ui.log').open('ab') as log:
        process = subprocess.Popen([executable, *args], env=env, stdout=subprocess.PIPE, stderr=log)
        try:
            yield process
        finally:
            if process.poll() is None: process.terminate()
            process.communicate(timeout=5)


def title(env, expected, process):
    deadline = time.monotonic() + 12
    while time.monotonic() < deadline:
        if process.poll() is not None: raise AssertionError(f'Rust browser exited: {process.returncode}')
        windows = subprocess.run(['xdotool', 'search', '--onlyvisible', '--pid', str(process.pid)],
                                 env=env, capture_output=True, text=True)
        if windows.returncode == 0:
            for window in windows.stdout.splitlines():
                name = subprocess.check_output(['xdotool', 'getwindowname', window], env=env, text=True).strip()
                if name == expected: return window
        time.sleep(.02)
    raise AssertionError(f'Browser did not publish title {expected!r}')


class RustBrowserTest(unittest.TestCase):
    def test_closing_a_browser_owner_preserves_its_dom_in_a_surviving_window(self):
        from test_keys import read_bytes
        with isolated_display() as display, profile(display) as env, fixture() as url:
            env.update(GDK_BACKEND='x11'); env.pop('WAYLAND_DISPLAY', None)
            with application(env, '-T', 'Keeper', '-o', '-', '-e', '/bin/sh', '-c', 'printf READY; exec cat') as app:
                self.assertEqual(read_bytes(app.stdout, 5), b'READY')
                keeper = title(env, 'Keeper', app)
                subprocess.run([APP, '--surf', url], env=env, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, check=True, timeout=5)
                browser = title(env, 'Fixture', app)
                subprocess.run(['xdotool', 'windowfocus', '--sync', browser], env=env, check=True)
                subprocess.run(['xdotool', 'type', '--clearmodifiers', 'live'], env=env, check=True)
                title(env, 'Edited:live', app)
                # Hit testing the real Cairo strip must switch pages through the windowless outline.
                subprocess.run(['xdotool', 'mousemove', '--window', browser, '5', '4', 'click', '1'],
                               env=env, check=True)
                deadline = time.monotonic() + 5
                def selected_title():
                    return subprocess.check_output(['xdotool', 'getwindowname', browser], env=env, text=True).strip()
                while selected_title() != 'Keeper':
                    self.assertLess(time.monotonic(), deadline, 'Cairo tab click did not select the terminal')
                    time.sleep(.02)
                subprocess.run(['xdotool', 'key', '--clearmodifiers', 'ctrl+9'], env=env, check=True)
                title(env, 'Edited:live', app)
                close_window(env, 'Edited:live')
                subprocess.run(['xdotool', 'windowfocus', '--sync', keeper], env=env, check=True)
                subprocess.run(['xdotool', 'key', '--clearmodifiers', 'ctrl+9'], env=env, check=True)
                title(env, 'Edited:live', app)
                subprocess.run(['xdotool', 'type', '--clearmodifiers', '!'], env=env, check=True)
                title(env, 'Edited:live!', app)

    def test_incomplete_launch_does_not_block_a_following_window(self):
        with isolated_display() as display, profile(display) as env, fixture() as url:
            env.update(GDK_BACKEND='x11'); env.pop('WAYLAND_DISPLAY', None)
            with application(env, '--attach') as app:
                title(env, 'Worminal Rust', app)
                digest = hashlib.sha256()
                for name in ('GDK_BACKEND', 'WAYLAND_DISPLAY', 'DISPLAY', 'WORMINAL_SHARED_SOCKET_SCOPE'):
                    value = os.fsencode(env.get(name, ''))
                    digest.update(len(value).to_bytes(8, 'big')); digest.update(value)
                address = f'\0worminal-rust-ui-{os.getuid()}-{digest.hexdigest()[:48]}'
                with socket.socket(socket.AF_UNIX) as incomplete:
                    incomplete.connect(address)
                    # Leave the first request waiting for its descriptor while a complete request arrives.
                    subprocess.run([APP, '--browser', url], env=env, check=True,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=3)
                    title(env, 'Fixture', app)

    def test_download_dialog_saves_response_bytes(self):
        with isolated_display() as display, profile(display) as env, fixture() as url:
            env.update(GDK_BACKEND='x11', GTK_USE_PORTAL='0')
            env.pop('WAYLAND_DISPLAY', None)
            destination = Path(env['XDG_STATE_HOME']).parent / 'download.txt'
            with application(env, '--browser', url) as app:
                window = title(env, 'Fixture', app)
                subprocess.run(['xdotool', 'windowfocus', '--sync', window], env=env, check=True)
                subprocess.run(['xdotool', 'key', '--clearmodifiers', 'ctrl+k'], env=env, check=True)
                subprocess.run(['xdotool', 'type', '--clearmodifiers', url + '/download'], env=env, check=True)
                subprocess.run(['xdotool', 'key', '--clearmodifiers', 'Return'], env=env, check=True)
                dialog = title(env, 'Save download', app)
                subprocess.run(['xdotool', 'windowfocus', '--sync', dialog], env=env, check=True)
                subprocess.run(['xdotool', 'key', '--clearmodifiers', 'ctrl+l', 'ctrl+a'], env=env, check=True)
                subprocess.run(['xdotool', 'type', '--clearmodifiers', str(destination)], env=env, check=True)
                subprocess.run(['xdotool', 'key', '--clearmodifiers', 'Return'], env=env, check=True)
                deadline = time.monotonic() + 5
                while not destination.exists():
                    if time.monotonic() >= deadline:
                        from Xlib import display as xdisplay
                        from visible_latency import capture_failure
                        connection = xdisplay.Display(env['DISPLAY'])
                        try: capture_failure(connection.screen().root)
                        finally: connection.close()
                    self.assertLess(time.monotonic(), deadline, 'native download did not complete')
                    time.sleep(.02)
                self.assertEqual(destination.read_bytes(), b'Worminal download fixture\n')

    def test_native_wayland_browser_presents_document_frames(self):
        ready = threading.Event()
        class Handler(Fixture):
            def do_GET(self):
                if self.path == '/report':
                    ready.set()
                    self.send_response(204); self.end_headers(); return
                body = (b'<!doctype html><body style="background:#123456;color:white">Rust Wayland browser'
                        b'<script>requestAnimationFrame(()=>requestAnimationFrame(()=>fetch("/report")))</script>')
                self.send_response(200); self.send_header('Content-Type', 'text/html')
                self.end_headers(); self.wfile.write(body)
        with wayland_display() as display, profile(display) as env, fixture(handler=Handler) as url:
            env.update(GDK_BACKEND='wayland', XDG_RUNTIME_DIR=display['XDG_RUNTIME_DIR'])
            env.pop('DISPLAY', None)
            with application(env, '--browser', url) as app:
                self.assertTrue(ready.wait(12), 'native Wayland browser did not run document frame callbacks')
                self.assertIsNone(app.poll())

    def test_module_replacement_pins_live_views_and_shares_only_profile_state(self):
        class Handler(Fixture):
            def do_GET(self):
                body = (b'<!doctype html><script>const shared=document.cookie.includes("shared=ok");'
                        b'document.cookie="shared=ok; path=/";'
                        b'document.title=(shared?"Shared":"Fresh")+":"+(sessionStorage.getItem("tab")||"new");'
                        b'sessionStorage.setItem("tab","old");</script>'
                        b'<input autofocus oninput="document.title=\'Edited:\'+this.value">')
                self.send_response(200); self.send_header('Content-Type', 'text/html')
                self.end_headers(); self.wfile.write(body)
        with (isolated_display() as display, profile(display) as env, fixture(handler=Handler) as url,
              service(env),
              tempfile.TemporaryDirectory(prefix='rust-browser-generations-', dir=ROOT / '.checks') as directory):
            env.update(GDK_BACKEND='x11'); env.pop('WAYLAND_DISPLAY', None)
            base = Path(directory)
            executable = str(copy_runtime(base, browser=True))
            module = base / 'build' / RUST_BROWSER_MODULE
            with application(env, '--browser', url, executable=executable) as app:
                first = title(env, 'Fresh:new', app)
                subprocess.run(['xdotool', 'windowfocus', '--sync', first], env=env, check=True)
                subprocess.run(['xdotool', 'type', '--clearmodifiers', 'kept'], env=env, check=True)
                title(env, 'Edited:kept', app)
                original = module.stat().st_ino
                shutil.copy2(module, module.with_suffix('.pending'))
                module.with_suffix('.pending').replace(module)
                self.assertNotEqual(original, module.stat().st_ino)
                subprocess.run([executable, '--browser', url], env=env, check=True,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=8)
                title(env, 'Shared:new', app)
                mappings = Path(f'/proc/{app.pid}/maps').read_text().splitlines()
                generations = {line.split()[4] for line in mappings if RUST_BROWSER_MODULE in line}
                self.assertEqual(len(generations), 2, 'new tabs did not load the newly published module inode')
                subprocess.run(['xdotool', 'windowfocus', '--sync', first], env=env, check=True)
                subprocess.run(['xdotool', 'type', '--clearmodifiers', '!'], env=env, check=True)
                title(env, 'Edited:kept!', app)

    def test_owner_forwarding_navigation_live_dom_and_daemon_replacement(self):
        with isolated_display() as display, profile(display) as env, fixture() as url, service(env) as daemon:
            env.update(GDK_BACKEND='x11')
            env.pop('WAYLAND_DISPLAY', None)
            with (ROOT / '.checks/rust-browser-broker.log').open('wb') as log:
                broker_check(env, url, log, daemon, rust=True)

    def test_selected_local_document_capability_and_explicit_reload(self):
        with (isolated_display() as display, profile(display) as env,
              tempfile.TemporaryDirectory(prefix='rust-browser-files-', dir=ROOT / '.checks') as directory):
            env.update(GDK_BACKEND='x11')
            env.pop('WAYLAND_DISPLAY', None)
            base = Path(directory)
            document = base / 'document.html'
            other = base / 'other.html'
            fifo = base / 'fifo'
            os.mkfifo(fifo)
            large = base / 'large'
            with large.open('wb') as output: output.truncate(32 * 1024 * 1024 + 1)
            other.write_text("<!doctype html><script>document.title='Other:'+typeof window.worminal</script>")
            # Real WebKit executes the same selected-document API as the independent explorer.
            script = r'''
                if (window !== top) { parent.postMessage(typeof window.worminal, '*'); }
                else (async () => {
                    const check = (ok, reason) => { if (!ok) throw Error(reason); };
                    check(typeof window.worminal === 'object', 'missing capability');
                    const path = decodeURIComponent(location.pathname);
                    const rows = await worminal.readDirectory(path.slice(0, path.lastIndexOf('/')));
                    check(rows.some(row => atob(row[0]) === 'document.html'), 'metadata');
                    const bytes = await worminal.readFile(path);
                    check(new TextDecoder().decode(bytes).startsWith('<!doctype html>'), 'file bytes');
                    for (const path of BAD_PATHS) {
                        let denied = false;
                        try { await worminal.readFile(path); } catch { denied = true; }
                        check(denied, 'unsafe file ' + path);
                    }
                    let denied = false;
                    try { const response = await fetch('worminal:///list?path=%2F'); denied = !response.ok; }
                    catch { denied = true; }
                    check(denied, 'missing credential');
                    await new Promise((resolve, reject) => {
                        addEventListener('message', event => {
                            if (event.data === 'undefined') resolve(); else reject(Error('subframe leak'));
                        }, {once:true});
                        const frame = document.createElement('iframe');
                        frame.setAttribute('sandbox', 'allow-scripts'); frame.src = location.href;
                        document.body.append(frame);
                    });
                    document.title = 'Local capability passed';
                })().catch(error => document.title='FAIL:'+error.message);
            '''.replace('BAD_PATHS', json.dumps(['/dev/zero', '/', 'relative', str(fifo), str(large)]))
            document.write_text('<!doctype html><body><script>' + script + '</script>')
            with application(env, '--browser', document.as_uri() + '?route=selected') as app:
                window = title(env, 'Local capability passed', app)
                document.write_text('<!doctype html><title>Explicit reload passed</title>')
                # Replacing the independent HTML file has no implicit reload side effect.
                self.assertEqual(subprocess.check_output(['xdotool', 'getwindowname', window], env=env,
                                                        text=True).strip(), 'Local capability passed')
                subprocess.run(['xdotool', 'windowfocus', '--sync', window], env=env, check=True)
                subprocess.run(['xdotool', 'key', '--clearmodifiers', 'ctrl+r'], env=env, check=True)
                title(env, 'Explicit reload passed', app)
                subprocess.run(['xdotool', 'key', '--clearmodifiers', 'ctrl+k'], env=env, check=True)
                subprocess.run(['xdotool', 'type', '--clearmodifiers', other.as_uri()], env=env, check=True)
                subprocess.run(['xdotool', 'key', '--clearmodifiers', 'Return'], env=env, check=True)
                title(env, 'Other:undefined', app)


if __name__ == '__main__':
    with (private_bus(os.environ) as env,
          patch.dict(os.environ, {'DBUS_SESSION_BUS_ADDRESS': env['DBUS_SESSION_BUS_ADDRESS']})):
        unittest.main(verbosity=1)
