"""Exercise lazy Rust WebKit modules through the actual native application on private displays."""
from runtime import CHECKS

from runtime import module_path
from runtime import runtime_binary

from contextlib import contextmanager
import json
import hashlib
import os
import shlex
import signal
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

from preview import audio, broker_check, Fixture, fixture, profile, service, wayland_display
from x11 import isolated_display, close_window
from test_window_state import private_bus
from runtime import copy_runtime, RUST_BROWSER_MODULE

ROOT = Path(__file__).resolve().parent.parent
APP = str(runtime_binary("worminal-rust"))
BROWSER_LOG = CHECKS / "rust-browser-ui.log"


@contextmanager
def application(env, *args, executable=APP):
    with BROWSER_LOG.open('ab') as log:
        process = subprocess.Popen([executable, *args], env=env, stdout=subprocess.PIPE, stderr=log)
        try:
            yield process
        finally:
            if process.poll() is None: process.terminate()
            process.communicate(timeout=5)


def title(env, expected, process):
    deadline = time.monotonic() + 12
    names = []
    while time.monotonic() < deadline:
        if process.poll() is not None: raise AssertionError(f'Rust browser exited: {process.returncode}')
        windows = subprocess.run(['xdotool', 'search', '--onlyvisible', '--pid', str(process.pid)],
                                 env=env, capture_output=True, text=True)
        if windows.returncode == 0:
            names = []
            for window in windows.stdout.splitlines():
                name = subprocess.check_output(['xdotool', 'getwindowname', window], env=env, text=True).strip()
                names.append(name)
                if name == expected: return window
        time.sleep(.02)
    raise AssertionError(f'Browser did not publish title {expected!r}; visible titles: {names!r}')


class RustBrowserTest(unittest.TestCase):
    def test_media_requires_a_gesture_and_decodes_into_private_audio(self):
        for name, mime, width in (('sample.mp3', 'audio/mpeg', 0), ('sample.wav', 'audio/wav', 0),
                                  ('sample.mp4', 'video/mp4', 96)):
            class MediaFixture(Fixture):
                def do_GET(self):
                    if self.path == '/media':
                        body = (ROOT / 'tests/fixtures/media' / name).read_bytes()
                        self.send_response(200); self.send_header('Content-Type', mime)
                        self.send_header('Content-Length', str(len(body))); self.end_headers()
                        self.wfile.write(body); return
                    self.html(b'<!doctype html><video id="m" src="/media"></video>'
                              b'<button autofocus onclick="m.play()">Play</button><script>'
                              b'm.ontimeupdate=()=>{if(m.currentTime>.1&&!m.error)'
                              b'document.title="Playing:"+m.videoWidth};'
                              b'm.play().then(()=>document.title="Unexpected autoplay",'
                              b'()=>document.title="Autoplay blocked")</script>')
            with (self.subTest(format=name), isolated_display() as private, profile(private) as base,
                  audio(base) as env, fixture(handler=MediaFixture) as url):
                env.update(GDK_BACKEND='x11'); env.pop('WAYLAND_DISPLAY', None)
                with application(env, '--browser', url) as app:
                    wid = title(env, 'Autoplay blocked', app)
                    subprocess.run(['xdotool', 'windowfocus', '--sync', wid, 'key', '--clearmodifiers', 'Return'],
                                   env=env, check=True)
                    title(env, f'Playing:{width}', app)

    def test_find_accepts_a_link_and_only_user_popups_open_tabs(self):
        requests = set()
        class FindFixture(Fixture):
            def do_GET(self):
                requests.add(self.path)
                if self.path != '/': return super().do_GET()
                body = (b'<!doctype html><title>Find popup</title><a href="/other" target="_blank">Popup</a>'
                        b'<script>window.open("/unwanted");setInterval(()=>{let s=getSelection().toString();'
                        b'if(s)document.title="Selected:"+s},20)</script>')
                self.html(body)
        with isolated_display() as private, profile(private) as env, fixture(handler=FindFixture) as url:
            env.update(GDK_BACKEND='x11'); env.pop('WAYLAND_DISPLAY', None)
            with application(env, '--browser', url) as app:
                wid = title(env, 'Find popup', app)
                subprocess.run(['xdotool', 'windowfocus', '--sync', wid, 'key', '--clearmodifiers', 'ctrl+f',
                                'type', '--clearmodifiers', 'Popup'], env=env, check=True)
                title(env, 'Selected:Popup', app)
                self.assertNotIn('/unwanted', requests, 'non-user popup reached the server')
                subprocess.run(['xdotool', 'key', '--clearmodifiers', 'Escape', 'Return'], env=env, check=True)
                title(env, 'Other', app)
                subprocess.run(['xdotool', 'key', '--clearmodifiers', 'ctrl+d'], env=env, check=True)
                title(env, 'Selected:Popup', app)

    def test_notification_permission_requires_an_explicit_native_answer(self):
        class PermissionFixture(Fixture):
            def do_GET(self):
                body = (b'<!doctype html><title>Permission ready</title><button autofocus '
                        b'onclick="Notification.requestPermission().then(p=>document.title=\'Permission:\'+p)">'
                        b'Request permission</button>')
                self.html(body)
        for answer, expected in ((['Return'], 'denied'), (['Tab', 'Return'], 'granted')):
            with (self.subTest(answer=expected), isolated_display() as private, profile(private) as env,
                  fixture(handler=PermissionFixture) as url):
                env.update(GDK_BACKEND='x11'); env.pop('WAYLAND_DISPLAY', None)
                with application(env, '--browser', url) as app:
                    wid = title(env, 'Permission ready', app)
                    subprocess.run(['xdotool', 'windowfocus', '--sync', wid, 'key', '--clearmodifiers', 'Return'],
                                   env=env, check=True)
                    deadline = time.monotonic() + 5
                    while True:
                        windows = subprocess.check_output(
                            ['xdotool', 'search', '--onlyvisible', '--pid', str(app.pid)],
                            env=env, text=True).splitlines()
                        dialogs = [window for window in windows if window != wid]
                        if dialogs: break
                        self.assertLess(time.monotonic(), deadline, 'permission request did not show a native dialog')
                        time.sleep(.02)
                    subprocess.run(
                        ['xdotool', 'windowfocus', '--sync', dialogs[-1], 'key', '--clearmodifiers', *answer],
                        env=env, check=True)
                    title(env, 'Permission:' + expected, app)

    def test_invalid_tls_never_delivers_the_document(self):
        delivered = threading.Event()
        class TLSFixture(Fixture):
            def do_GET(self):
                delivered.set(); super().do_GET()
        with isolated_display() as private, profile(private) as env, fixture(tls=True, handler=TLSFixture) as url:
            env.update(GDK_BACKEND='x11'); env.pop('WAYLAND_DISPLAY', None)
            with application(env, '--browser', url) as app:
                # Error chrome keeps the failed URL as its title; copy the actual rendered error document.
                wid = title(env, url + '/', app)
                deadline = time.monotonic() + 5
                while True:
                    subprocess.run(['xdotool', 'windowfocus', '--sync', wid, 'key', '--clearmodifiers',
                                    'ctrl+a', 'ctrl+c'], env=env, check=True)
                    copied = subprocess.run(['xclip', '-selection', 'clipboard', '-o'], env=env,
                                            capture_output=True, timeout=3).stdout
                    if b'Certificate validation failed' in copied: break
                    self.assertLess(time.monotonic(), deadline, 'TLS failure did not paint the native error document')
                    time.sleep(.02)
                self.assertFalse(delivered.is_set(), 'invalid TLS document reached HTTP handling')

    def test_renderer_failure_recovers_on_explicit_reload(self):
        from perf_metrics import descendants
        from visible_latency import wait_for
        class RecoveryFixture(Fixture):
            loads = 0
            def do_GET(self):
                if self.path != '/':
                    self.send_error(404); return
                type(self).loads += 1
                self.html((f'<!doctype html><title>Renderer {self.loads}</title>'
                           '<input autofocus oninput="document.title=\'Edited:\'+this.value">').encode())
        with isolated_display() as private, profile(private) as env, fixture(handler=RecoveryFixture) as url:
            env.update(GDK_BACKEND='x11'); env.pop('WAYLAND_DISPLAY', None)
            with application(env, '--browser', url) as app:
                wid = title(env, 'Renderer 1', app)
                renderers = [pid for pid in descendants(app.pid)
                             if Path(f'/proc/{pid}/comm').read_text().startswith('WebKitWebProces')]
                self.assertEqual(len(renderers), 1)
                offset = BROWSER_LOG.stat().st_size
                identity = os.pidfd_open(renderers[0])
                try: signal.pidfd_send_signal(identity, signal.SIGKILL)
                finally: os.close(identity)
                # The owner reports this after answering WebKit's termination signal and showing recovery chrome.
                stopped = f'worminal: renderer stopped in owner {app.pid} '.encode()
                wait_for(lambda: stopped in BROWSER_LOG.read_bytes()[offset:])
                subprocess.run(['xdotool', 'windowfocus', '--sync', wid, 'key', '--clearmodifiers', 'ctrl+r'],
                               env=env, check=True)
                title(env, 'Renderer 2', app)
                subprocess.run(['xdotool', 'type', '--clearmodifiers', 'recovered'], env=env, check=True)
                title(env, 'Edited:recovered', app)

    def test_browser_conversion_preserves_terminal_background(self):
        from Xlib import X, display
        from test_keys import read_bytes
        with (isolated_display() as private, profile(private) as env,
              tempfile.TemporaryDirectory(prefix='rust-browser-palette-', dir=CHECKS) as directory):
            env.update(GDK_BACKEND='x11'); env.pop('WAYLAND_DISPLAY', None)
            document = Path(directory) / 'blank.html'
            document.write_text('<!doctype html><title>Palette browser</title>')
            # Wait for real input so conversion cannot overtake parsing the PTY's palette change.
            command = ("printf '\\033]11;#1163c9\\007READY'; read ignored; exec " +
                       shlex.join([APP, '--new-browser-tab', str(document)]))
            with application(env, '-T', 'Palette terminal', '-o', '-', '-e', '/bin/sh', '-c', command) as app:
                ready = b'\x1b]11;#1163c9\x07READY'
                self.assertEqual(read_bytes(app.stdout, len(ready)), ready)
                wid = title(env, 'Palette terminal', app)
                subprocess.run(['xdotool', 'windowfocus', '--sync', wid, 'key', 'Return'], env=env, check=True)
                wid = title(env, 'Palette browser', app)
                server = display.Display(env['DISPLAY'])
                try:
                    window = server.create_resource_object('window', int(wid))
                    deadline = time.monotonic() + 5
                    while True:
                        geometry = window.get_geometry()
                        pixel = window.get_image(geometry.width // 2, geometry.height // 2,
                                                 1, 1, X.ZPixmap, 0xffffffff).data
                        if tuple(pixel[:3][::-1]) == (17, 99, 201): break
                        self.assertLess(time.monotonic(), deadline, 'browser lost the tab palette background')
                        time.sleep(.02)
                finally:
                    server.close()

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

    def test_module_replacement_reloads_views_and_transfers_cookie_profile(self):
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
              tempfile.TemporaryDirectory(prefix='rust-browser-generations-', dir=CHECKS) as directory):
            env.update(GDK_BACKEND='x11'); env.pop('WAYLAND_DISPLAY', None)
            base = Path(directory)
            executable = str(copy_runtime(base, browser=True))
            module = module_path(RUST_BROWSER_MODULE, prefix=base)
            with application(env, '--browser', url, executable=executable) as app:
                first = title(env, 'Fresh:new', app)
                subprocess.run(['xdotool', 'windowfocus', '--sync', first], env=env, check=True)
                subprocess.run(['xdotool', 'type', '--clearmodifiers', 'kept'], env=env, check=True)
                title(env, 'Edited:kept', app)
                original = module.stat().st_ino
                healthy = module.read_bytes()
                pending = module.with_suffix('.pending')
                pending.write_bytes(b'not a shared library')
                pending.replace(module)
                time.sleep(2)
                title(env, 'Edited:kept', app)
                pending.write_bytes(healthy)
                pending.replace(module)
                original = module.stat().st_ino
                shutil.copy2(module, module.with_suffix('.pending'))
                module.with_suffix('.pending').replace(module)
                self.assertNotEqual(original, module.stat().st_ino)
                replacement = None
                try:
                    deadline = time.monotonic() + 20
                    while time.monotonic() < deadline:
                        found = subprocess.run(['xdotool', 'search', '--onlyvisible', '--name', '^Shared:new$'],
                                               env=env, capture_output=True, text=True)
                        if found.returncode == 0:
                            replacement = int(subprocess.check_output(
                                ['xdotool', 'getwindowpid', found.stdout.splitlines()[0]], env=env, text=True))
                            break
                        time.sleep(.03)
                    self.assertIsNotNone(replacement, 'Updated browser did not reload with its session cookie')
                    self.assertNotEqual(replacement, app.pid)
                    app.wait(timeout=5)
                    mappings = Path(f'/proc/{replacement}/maps').read_text().splitlines()
                    generations = {line.split()[4] for line in mappings if RUST_BROWSER_MODULE in line}
                    self.assertEqual(generations, {str(module.stat().st_ino)})
                finally:
                    if replacement is not None:
                        os.kill(replacement, signal.SIGTERM)
                        time.sleep(.3)

    def test_owner_forwarding_navigation_live_dom_and_daemon_replacement(self):
        with isolated_display() as display, profile(display) as env, fixture() as url, service(env) as daemon:
            env.update(GDK_BACKEND='x11')
            env.pop('WAYLAND_DISPLAY', None)
            with (CHECKS / "rust-browser-broker.log").open('wb') as log:
                broker_check(env, url, log, daemon, rust=True)

    def test_selected_local_document_capability_and_explicit_reload(self):
        with (isolated_display() as display, profile(display) as env,
              tempfile.TemporaryDirectory(prefix='rust-browser-files-', dir=CHECKS) as directory):
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
