"""Check automatic owner replacement on a private X server with a copied installation."""
import errno
import os
from pathlib import Path
import shutil
import signal
import select
import shlex
import json
import subprocess
import tempfile
import time
import unittest
import sys
from contextlib import contextmanager, ExitStack
from preview import profile, service, fixture, Fixture
from runtime import ROOT, CHECKS, copy_runtime, module_path, runtime_binary, RUST_FRONTEND_MODULE, RUST_BROWSER_MODULE
from x11 import isolated_display, close_window, normal_windows, window_manager


def wait(check, message):
    deadline = time.monotonic() + 25
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(.03)
    raise AssertionError(message() if callable(message) else message)


def open_process(pid):
    """A pidfd for `pid`, waiting out a dying process whose leader exited before its other threads."""
    deadline = time.monotonic() + 5
    while True:
        try:
            return os.pidfd_open(pid)
        except OSError as failure:
            # pidfd_open reports EINVAL for a thread group without its leader; it ends as ESRCH shortly.
            if failure.errno != errno.EINVAL or time.monotonic() > deadline: raise
            time.sleep(.02)


def stop_fixture(env):
    """Wait for this private scope's owners and browser workers before deleting mapped files."""
    identities = []
    scope = ('WORMINAL_SHARED_SOCKET_SCOPE='+env['WORMINAL_SHARED_SOCKET_SCOPE']).encode()
    for process in Path('/proc').iterdir():
        if not process.name.isdecimal() or int(process.name) == os.getpid(): continue
        identity = None
        try:
            identity = open_process(int(process.name))
            if scope not in (process/'environ').read_bytes().split(b'\0'):
                continue
            signal.pidfd_send_signal(identity, signal.SIGTERM)
            identities.append(identity)
            identity = None
        except (ProcessLookupError, FileNotFoundError, PermissionError):
            continue
        finally:
            if identity is not None:
                os.close(identity)
    for identity in identities:
        try:
            poll = select.poll()
            poll.register(identity, select.POLLIN)
            if not poll.poll(5000):
                signal.pidfd_send_signal(identity, signal.SIGKILL)
                if not poll.poll(5000): raise AssertionError('Private fixture process did not exit')
        finally:
            os.close(identity)


@contextmanager
def retile_on_map(env, original):
    """Model a tiling WM resizing old clients as replacement clients become visible."""
    # A fresh interpreter also isolates Xlib's select()-limited socket descriptors
    # from the browser fixture's process-wide descriptor allocation.
    with tempfile.TemporaryDirectory(prefix='retile-', dir=CHECKS) as directory:
        ready, resized = Path(directory)/'ready', Path(directory)/'resized'
        worker = subprocess.Popen([sys.executable, str(Path(__file__).with_name('tiling_wm.py')),
                                   str(ready), str(resized), *original], env=env)
        try:
            wait(ready.exists, 'Private retiling observer did not start')
            yield resized
        finally:
            worker.terminate()
            worker.wait(timeout=5)
def replace_browser_transfer(base, browser, import_body):
    """Install a private ABI-compatible browser shim around the current implementation."""
    healthy = base/'healthy-browser.so'
    shutil.copy2(browser, healthy)
    shim = base/'profile-transfer.c'
    shim.write_text('#include <dlfcn.h>\n#include <stdio.h>\n#include <unistd.h>\n'
        '#include "rust_browser_module.h"\n'
        f'static void *library(void) {{ static void *h; if(!h) h=dlopen({json.dumps(str(healthy))},'
        'RTLD_NOW|RTLD_LOCAL); return h; }\n'
        'const WorminalRustBrowser *worminal_rust_browser_module(uint32_t v) {'
        'const WorminalRustBrowser *(*entry)(uint32_t)=dlsym(library(),"worminal_rust_browser_module");'
        'return entry(v); }\n'
        'int32_t worminal_browser_profile_transfer_v1(void *p,const uint8_t *b,uint64_t n,'
        'WorminalBrowserProfileReply reply,void *ctx) {' + import_body +
        'WorminalBrowserProfileTransfer transfer=dlsym(library(),"worminal_browser_profile_transfer_v1");'
        'return transfer(p,b,n,reply,ctx); }\n')
    pending = browser.with_suffix('.pending')
    subprocess.run(['cc', '-shared', '-fPIC', '-I', str(ROOT/'src/gtk'), str(shim), '-ldl', '-o', str(pending)],
                   check=True)
    pending.replace(browser)
    return healthy


def gate_candidate(base, executable):
    """Pause the actual native candidate after the donor captured its provisional state."""
    ready = base/'ready'
    ready.mkdir()
    healthy = copy_runtime(ready, browser=True)
    gate, waiting = base/'continue-update', base/'candidate-waiting'
    pending = executable.with_suffix('.pending')
    pending.write_text('#!/bin/sh\n'
        'if [ "$1" = "--replace-owner" ]; then\n'
        f'  : > {shlex.quote(str(waiting))}\n'
        f'  while [ ! -e {shlex.quote(str(gate))} ]; do sleep .02; done\n'
        'fi\n'
        f'exec {shlex.quote(str(healthy))} "$@"\n')
    pending.chmod(0o755)
    pending.replace(executable)
    wait(waiting.exists, 'Native candidate did not enter preparation')
    return healthy, gate


def gated_master(base, env):
    """Share the private native master transport used by handoff fence regressions."""
    gate, waiting = base/'release-input', base/'input-waiting'
    daemon = str(runtime_binary('worminald'))
    scope = env['WORMINAL_SHARED_SOCKET_SCOPE']+'-remote'
    wrapper = base/'gated-daemon'
    wrapper.write_text('#!/usr/bin/python3\nimport runpy\n'
        f'runpy.run_path({str(ROOT/"tests/fixtures/gated_master_bridge.py")!r},run_name="__main__")\n')
    wrapper.chmod(0o700)
    ssh = base/'ssh'
    ssh.write_text('#!/usr/bin/python3\nimport os,sys\n'
        f'os.environ["WORMINAL_SHARED_SOCKET_SCOPE"]={scope!r}\n'
        f'sys.path.insert(0,{str(ROOT/"tests/fixtures")!r})\n'
        'from master_ssh import run\n'
        f'run({str(wrapper)!r})\n')
    ssh.chmod(0o700)
    env = {**env, 'GDK_BACKEND': 'x11', 'PATH': str(base)+os.pathsep+env['PATH'],
           'WORMINAL_TEST_GATE_DAEMON': daemon, 'WORMINAL_TEST_PYTHON_DIR': str(ROOT/'tests'),
           'WORMINAL_TEST_INPUT_WAITING': str(waiting), 'WORMINAL_TEST_INPUT_GATE': str(gate)}
    env.pop('DBUS_SESSION_BUS_ADDRESS', None)
    remote = {**env, 'WORMINAL_SHARED_SOCKET_SCOPE': scope}
    return env, remote


class ReplacementTest(unittest.TestCase):
    def test_browser_reloads_with_session_and_persistent_cookies(self):
        requests = []
        class Cookies(Fixture):
            def do_GET(self):
                cookie = self.headers.get('Cookie', '')
                requests.append(cookie)
                body = ("<!doctype html><title>Loading</title><script>"
                        "const retained=localStorage.getItem('fixture')==='saved';"
                        "localStorage.setItem('fixture','saved');"
                        f"document.title={'true' if 'session=alive' in cookie and 'persistent=alive' in cookie else 'false'}"
                        "&&retained?'Retained':'Initial';</script>").encode()
                self.send_response(200)
                self.send_header('Set-Cookie', 'session=alive; Path=/; HttpOnly; SameSite=Lax')
                self.send_header('Set-Cookie', 'persistent=alive; Path=/; Max-Age=3600; SameSite=Strict')
                self.send_header('Content-Type', 'text/html')
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.wfile.write(body)
        with isolated_display() as display, profile(display) as env, service(env), fixture(handler=Cookies) as url, \
                tempfile.TemporaryDirectory(prefix='replacement-browser-', dir=CHECKS) as directory:
            executable = copy_runtime(directory, browser=True)
            env = {**env, 'GDK_BACKEND': 'x11', 'GIO_USE_VFS': 'local'}
            env.pop('DBUS_SESSION_BUS_ADDRESS', None)
            log_path = Path(directory) / 'owner.log'
            def named(name):
                result = subprocess.run(['xdotool', 'search', '--onlyvisible', '--name', '^'+name+'$'],
                                        env=env, capture_output=True, text=True)
                return result.stdout.splitlines() if result.returncode == 0 else []
            with log_path.open('wb') as log:
                owner = subprocess.Popen([str(executable), '--browser', url], env=env, stdout=log, stderr=log)
                try:
                    wait(lambda: named('Initial'), 'Initial browser page did not load')
                    # A valid candidate module fails only when importing the cookie jar: the old
                    # profile has already retired, so this proves profile/cookie rollback rather
                    # than merely rejecting a malformed candidate before READY.
                    browser = module_path(RUST_BROWSER_MODULE, prefix=Path(directory))
                    marker = Path(directory) / 'import-attempted'
                    healthy = replace_browser_transfer(Path(directory), browser,
                        f'if(b){{FILE *f=fopen({json.dumps(str(marker))},"w");if(f)fclose(f);'
                        'reply(ctx,1,0,0);return 1;}')
                    pending_browser = browser.with_suffix('.pending')
                    wait(marker.exists, 'Candidate never attempted post-retirement cookie import')
                    wait(lambda: named('Retained'), 'Old browser did not reload after post-retirement failure')
                    self.assertIsNone(owner.poll(), 'Rollback closed the healthy old owner')
                    shutil.copy2(healthy, pending_browser)
                    pending_browser.replace(browser)
                    module = module_path(RUST_FRONTEND_MODULE, prefix=Path(directory))
                    pending = module.with_suffix('.pending')
                    shutil.copy2(module, pending)
                    pending.replace(module)
                    wait(lambda: named('Retained'), 'Cookies/storage missing after replacement: '+log_path.read_text(errors='replace'))
                    self.assertTrue(any('session=alive' in item and 'persistent=alive' in item for item in requests))
                    wait(lambda: owner.poll() is not None, 'Old browser owner did not exit')
                finally:
                    for window in named('Retained') + named('Initial'):
                        identity = subprocess.check_output(['xdotool', 'getwindowpid', window], env=env, text=True)
                        os.kill(int(identity), 15)
                    if owner.poll() is None: owner.terminate()
                    owner.wait(timeout=5)
                    stop_fixture(env)

    def test_a_split_browser_beside_the_focused_terminal_survives_repeated_replacement(self):
        # Activation hosts only the active tab, so a split's other browser stayed a "Show browser here"
        # placeholder after replacement, and that owner kept its adopted profile lease unused. It then reported
        # no lease to the next candidate, whose own profile open met the lease still held by the donor:
        # "Rust browser profile is already open in another owner".
        from Xlib import X, display as xdisplay, error
        sys.path.insert(0, str(ROOT / 'tests'))
        from visible_latency import image_bytes
        white = (255, 255, 255)
        with isolated_display() as display, profile(display) as env, service(env), ExitStack() as cleanup, \
                tempfile.TemporaryDirectory(prefix='replacement-split-', dir=CHECKS) as directory:
            executable = copy_runtime(directory, browser=True)
            env = {**env, 'GDK_BACKEND': 'x11', 'GIO_USE_VFS': 'local'}
            env.pop('DBUS_SESSION_BUS_ADDRESS', None)
            page = Path(directory) / 'page.html'
            page.write_text('<!doctype html><title>page</title><body style="margin:0;background:#fff">')
            server = xdisplay.Display(env['DISPLAY'])
            cleanup.callback(server.close)
            def windows():
                result = subprocess.run(['xdotool', 'search', '--onlyvisible', '--class', 'SplitFixture'],
                                        env=env, capture_output=True, text=True)
                return result.stdout.splitlines() if result.returncode == 0 else []
            def pid(window):
                result = subprocess.run(['xdotool', 'getwindowpid', window], env=env, capture_output=True, text=True)
                return result.stdout.strip() if result.returncode == 0 else None
            def pixel(fx):
                window = windows()
                if not window: return None
                try:
                    resource = server.create_resource_object('window', int(window[0]))
                    geometry = resource.get_geometry()
                    data = image_bytes(resource.get_image(int(geometry.width * fx), geometry.height * 3 // 4,
                                                          1, 1, X.ZPixmap, 0xffffffff))
                except error.BadMatch:
                    return None
                return tuple(data[:3][::-1])
            def sized():
                # Pixels are read on screen: the default window is wider than the private display.
                xdo('windowsize', windows()[0], '900', '500')
                return server.create_resource_object('window', int(windows()[0])).get_geometry().width == 900
            def split_shown():
                return pixel(.75) == white and pixel(.25) not in (white, None)
            xdo = lambda *args: subprocess.run(['xdotool', *args], env=env, check=True)
            api = lambda *args: subprocess.check_output([str(runtime_binary('worminald')), 'api', *args],
                                                        env=env, text=True)
            log_path = Path(directory) / 'owner.log'
            with log_path.open('wb') as log:
                owner = subprocess.Popen([str(executable), '-c', 'SplitFixture', '-o', '-'], env=env,
                                         stdout=log, stderr=log)
                try:
                    wait(windows, 'The window did not open')
                    wait(sized, 'The window did not resize')
                    xdo('windowfocus', '--sync', windows()[0])
                    api('open', '--browser', page.as_uri())
                    wait(lambda: len(json.loads(api('list'))['tabs']) == 2, 'No browser tab opened')
                    xdo('key', '--clearmodifiers', 'alt+2')
                    wait(lambda: pixel(.5) == white, 'The browser page was not shown')
                    # The browser moves into its own pane; the terminal pane then takes focus.
                    xdo('key', '--clearmodifiers', 'alt+shift+Right')
                    wait(split_shown, 'The browser did not split off beside the terminal')
                    xdo('key', '--clearmodifiers', 'ctrl+shift+Left')
                    time.sleep(.5)
                    module = module_path(RUST_FRONTEND_MODULE, prefix=Path(directory))
                    for attempt in range(2):
                        previous = pid(windows()[0])
                        pending = module.with_suffix('.pending')
                        shutil.copy2(module, pending)
                        pending.replace(module)
                        # Replacement can destroy the listed window between two listings; read it once.
                        wait(lambda: (current := windows()) and pid(current[0]) not in (previous, None),
                             f'Replacement {attempt + 1} did not take over')
                        wait(sized, 'The replaced window did not resize')
                        wait(split_shown, lambda: f'Replacement {attempt + 1} lost the split browser: '
                             + log_path.read_text(errors='replace'))
                finally:
                    for window in windows():
                        identity = pid(window)
                        if identity: os.kill(int(identity), 15)
                    if owner.poll() is None: owner.terminate()
                    owner.wait(timeout=5)
                    stop_fixture(env)

    def test_replacement_keeps_the_window_on_its_virtual_desktop(self):
        # A replacement window maps on the current desktop unless the donor's desktop is applied to it.
        with isolated_display() as display, profile(display) as env, service(env), ExitStack() as cleanup, \
                tempfile.TemporaryDirectory(prefix='replacement-desktop-', dir=CHECKS) as directory:
            manager = cleanup.enter_context(window_manager(display))
            if manager is None: self.skipTest('Install xfwm4 to check virtual desktops')
            executable = copy_runtime(directory)
            env = {**env, 'GDK_BACKEND': 'x11'}
            env.pop('DBUS_SESSION_BUS_ADDRESS', None)
            xdo = lambda *args: subprocess.run(['xdotool', *args], env=env, capture_output=True, text=True).stdout
            def client():
                # The managed window with its owner's pid, as (pid, desktop).
                for window in xdo('search', '--class', 'DesktopFixture').split():
                    desktop = xdo('get_desktop_for_window', window).strip()
                    if desktop.isdigit():
                        return xdo('getwindowpid', window).strip(), desktop
                return None
            log_path = Path(directory) / 'owner.log'
            with log_path.open('wb') as log:
                owner = subprocess.Popen([str(executable), '-c', 'DesktopFixture', '-o', '-'], env=env,
                                         stdout=log, stderr=log)
                try:
                    window = wait(lambda: xdo('search', '--onlyvisible', '--class', 'DesktopFixture').split(),
                                  'The window did not open')[0]
                    xdo('set_desktop_for_window', window, '2')
                    wait(lambda: client() and client()[1] == '2', 'The window manager did not move the window')
                    previous = client()[0]
                    module = module_path(RUST_FRONTEND_MODULE, prefix=Path(directory))
                    pending = module.with_suffix('.pending')
                    shutil.copy2(module, pending)
                    pending.replace(module)
                    wait(lambda: client() and client()[0] not in (previous, ''), 'The replacement did not take over')
                    time.sleep(.5)
                    self.assertEqual(client()[1], '2', log_path.read_text(errors='replace'))
                    self.assertEqual(xdo('get_desktop').strip(), '0')
                finally:
                    if owner.poll() is None: owner.terminate()
                    owner.wait(timeout=5)
                    for found in xdo('search', '--class', 'DesktopFixture').split():
                        identity = xdo('getwindowpid', found).strip()
                        if identity.isdigit(): os.kill(int(identity), 15)
                    stop_fixture(env)

    def test_close_and_geometry_edits_during_commit_reach_the_new_owner(self):
        class ClosePage(Fixture):
            def do_GET(self):
                self.send_response(200)
                self.send_header('Set-Cookie', 'session=alive; Path=/; HttpOnly')
                self.send_header('Content-Type', 'text/html')
                self.end_headers()
                self.wfile.write(b'<!doctype html><title>CloseDuringCommit</title>')
        with isolated_display() as display, profile(display) as env, service(env), fixture(handler=ClosePage) as url, \
                tempfile.TemporaryDirectory(prefix='replacement-commit-', dir=CHECKS) as directory:
            base = Path(directory)
            executable = copy_runtime(base, browser=True)
            env = {**env, 'GDK_BACKEND': 'x11', 'GIO_USE_VFS': 'local'}
            env.pop('DBUS_SESSION_BUS_ADDRESS', None)
            gate, marker, shell = base/'continue-import', base/'import-waiting', base/'shell'
            log_path = CHECKS/'window-replacement-commit.log'
            def named(title):
                result = subprocess.run(['xdotool', 'search', '--onlyvisible', '--name', '^'+title+'$'],
                                        env=env, capture_output=True, text=True)
                return result.stdout.splitlines() if result.returncode == 0 else []
            with log_path.open('wb') as log:
                owner = subprocess.Popen([str(executable), '-T', 'KeptDuringCommit', '-e', '/bin/sh', '-c',
                    f'echo $$ > {shlex.quote(str(shell))}; exec cat'], env=env, stdout=log, stderr=log)
                try:
                    wait(lambda: shell.exists() and named('KeptDuringCommit'), 'Terminal did not start')
                    shell_pid = shell.read_text().strip()
                    subprocess.run([str(executable), '--browser', url], env=env, stdout=log, stderr=log,
                                   timeout=5, check=True)
                    wait(lambda: named('CloseDuringCommit'), 'Browser did not start')
                    replace_browser_transfer(base, module_path(RUST_BROWSER_MODULE, prefix=base),
                        f'if(b){{FILE *f=fopen({json.dumps(str(marker))},"w");if(f)fclose(f);'
                        f'while(access({json.dumps(str(gate))},F_OK)!=0)usleep(10000);}}')
                    wait(marker.exists, 'Candidate did not enter the post-retirement commit phase')
                    close_window(env, 'CloseDuringCommit')
                    time.sleep(.1)
                    self.assertTrue(named('CloseDuringCommit'), 'Donor was destroyed before the replacement committed')
                    terminal = named('KeptDuringCommit')[0]
                    subprocess.run(['xdotool', 'windowmove', terminal, '77', '91', 'windowsize', terminal, '700', '440'],
                                   env=env, check=True)
                    time.sleep(.1)
                    geometry = subprocess.check_output(['xdotool', 'getwindowgeometry', '--shell', terminal],
                                                       env=env, text=True).split('X=', 1)[1]
                    queued_marker = base/'queued-shell'
                    queued = subprocess.Popen([str(executable), '-T', 'QueuedDuringCommit', '-e', '/bin/sh', '-c',
                        f'echo $$ >> {shlex.quote(str(queued_marker))}; exec cat'],
                        env=env, stdout=log, stderr=log)
                    try:
                        # The old fixed five-second launcher timeout lost the reply during
                        # a valid commit gate. Requests must await the new owner exactly once.
                        time.sleep(5.2)
                        self.assertIsNone(queued.poll(), 'Queued launch timed out during a valid handoff')
                        self.assertFalse(queued_marker.exists(), 'Paused donor accepted a new window')
                        gate.touch()
                        self.assertEqual(queued.wait(timeout=10), 0)
                        wait(queued_marker.exists, 'Queued launch did not reach the committed owner')
                        self.assertEqual(len(queued_marker.read_text().splitlines()), 1)
                    finally:
                        gate.touch()
                        if queued.poll() is None:
                            queued.terminate()
                        queued.wait(timeout=5)
                    wait(lambda: owner.poll() is not None and named('KeptDuringCommit'),
                         'Update did not commit: '+log_path.read_text(errors='replace'))
                    wait(lambda: not named('CloseDuringCommit'), 'Queued window close was lost during handoff')
                    current = named('KeptDuringCommit')[0]
                    self.assertEqual(subprocess.check_output(['xdotool', 'getwindowgeometry', '--shell', current],
                                     env=env, text=True).split('X=', 1)[1], geometry)
                    os.kill(int(shell_pid), 0)
                    self.assertEqual(shell.read_text().strip(), shell_pid)
                finally:
                    gate.touch()
                    stop_fixture(env)
                    owner.wait(timeout=5)

    def test_multihost_links_and_selected_remote_tab_survive_replacement(self):
        self.multihost_replacement(reconnect=False)

    def test_replacement_retries_after_remote_link_recovers(self):
        self.multihost_replacement(reconnect=True)

    def multihost_replacement(self, *, reconnect):
        with isolated_display() as display, profile(display) as env, \
                tempfile.TemporaryDirectory(prefix='replacement-links-', dir=CHECKS) as directory:
            executable = copy_runtime(directory, browser=True)
            base = Path(directory)
            scopes = {host: env['WORMINAL_SHARED_SOCKET_SCOPE']+'-'+host for host in ('remotea', 'remoteb')}
            ssh = base/'ssh'
            daemon = str(runtime_binary('worminald'))
            ssh.write_text('#!/usr/bin/python3\nimport os,sys,time\nfrom pathlib import Path\n'
                'assert sys.argv[1:4]==["-T","-C","-oConnectTimeout=10"]\n'
                f'scopes={scopes!r}\n'
                'assert sys.argv[4] in scopes\n'
                f'base=Path({str(base)!r})\n'
                '(base/("bridge-"+sys.argv[4])).write_text(str(os.getpid()))\n'
                'while (base/"pause-bridge").exists(): time.sleep(.02)\n'
                'os.environ["WORMINAL_SHARED_SOCKET_SCOPE"]=scopes[sys.argv[4]]\n'
                f'sys.path.insert(0,{str(ROOT / "tests/fixtures")!r})\n'
                'from master_ssh import run\n'
                f'run({daemon!r})\n')
            ssh.chmod(0o700)
            env = {**env, 'GDK_BACKEND': 'x11', 'PATH': str(base)+os.pathsep+env['PATH']}
            env.pop('DBUS_SESSION_BUS_ADDRESS', None)
            remotea = {**env, 'WORMINAL_SHARED_SOCKET_SCOPE': scopes['remotea']}
            remoteb = {**env, 'WORMINAL_SHARED_SOCKET_SCOPE': scopes['remoteb']}
            def visible():
                found = subprocess.run(['xdotool','search','--onlyvisible','--class','MultiHostFixture'],
                                       env=env,capture_output=True,text=True)
                return found.stdout.splitlines() if found.returncode==0 else []
            def owner():
                windows = visible()
                # Replacement can destroy the listed window before its PID is read.
                if not windows: return None
                result = subprocess.run(['xdotool','getwindowpid',windows[0]],env=env,capture_output=True,text=True)
                return result.stdout.strip() if result.returncode == 0 else None
            # The local daemon owns the master links, so it runs with this fake ssh on its PATH.
            with service({**env, 'WORMINAL_GUI': str(executable)}), service(remotea), service(remoteb), \
                    (base/'owner.log').open('wb') as log:
                process = None
                try:
                    for host in ('local', 'remotea', 'remoteb'):
                        marker = base/host
                        command = [str(executable),'-c','MultiHostFixture','-T',host,
                                   *([] if host=='local' else ['--master',host]),
                                   '-e','/bin/sh','-c',f'echo $$ > {marker}; exec cat']
                        launch = subprocess.Popen(command,env=env,stdout=log,stderr=log)
                        if host=='local': process = launch
                        else: self.assertEqual(launch.wait(timeout=5),0)
                        wait(marker.exists,'Host tab did not launch: '+host)
                    wait(lambda: len(visible())==1,'Remote hosts opened extra windows')
                    previous = owner()
                    identities = {host:(base/host).read_text().strip() for host in ('local','remotea','remoteb')}
                    # The local daemon owns master links: a window update while one reconnects neither
                    # waits for it nor disturbs its tabs.
                    if reconnect:
                        gate = base/'pause-bridge'
                        bridge = base/'bridge-remotea'
                        previous_bridge = bridge.read_text()
                        gate.touch()
                        os.kill(int(previous_bridge), signal.SIGTERM)
                        wait(lambda: bridge.read_text() != previous_bridge, 'Remote link did not reconnect')
                    pending = module_path(RUST_FRONTEND_MODULE,prefix=base).with_suffix('.pending')
                    shutil.copy2(module_path(RUST_FRONTEND_MODULE,prefix=base),pending)
                    pending.replace(module_path(RUST_FRONTEND_MODULE,prefix=base))
                    def replacement_owner():
                        current = owner()
                        return current if current and current != previous else None
                    wait(replacement_owner,'Multi-host window did not replace')
                    if reconnect:
                        gate.unlink()
                    self.assertEqual(len(visible()),1)
                    name = subprocess.check_output(['xdotool','getwindowname',visible()[0]],env=env,text=True).strip()
                    # A master's tabs appearing or returning never move the selection.
                    self.assertEqual(name,'remoteb','Selected remote tab was not restored')
                    for host, pid in identities.items():
                        os.kill(int(pid),0)
                        self.assertEqual((base/host).read_text().strip(),pid,'Update replayed remote launch')
                finally:
                    stop_fixture(env)
                    if process is not None: process.wait(timeout=5)

    def test_changes_during_preparation_commit_the_latest_window_state(self):
        with isolated_display() as display, profile(display) as env, service(env), \
                tempfile.TemporaryDirectory(prefix='replacement-edits-', dir=CHECKS) as directory:
            base = Path(directory)
            executable = copy_runtime(base, browser=True)
            env = {**env, 'GDK_BACKEND': 'x11', 'GIO_USE_VFS': 'local'}
            env.pop('DBUS_SESSION_BUS_ADDRESS', None)
            log_path = CHECKS/'window-replacement-edits.log'
            processes = []
            def windows():
                return normal_windows(env, 'AtomicEditFixture')
            def names():
                result = {}
                for window in windows():
                    title = subprocess.run(['xdotool', 'getwindowname', window], env=env,
                                           capture_output=True, text=True)
                    if title.returncode == 0:
                        result[title.stdout.strip()] = window
                return result
            def pid(window):
                result = subprocess.run(['xdotool', 'getwindowpid', window], env=env, capture_output=True, text=True)
                return result.stdout.strip() if result.returncode == 0 else None
            def geometry():
                return {name: subprocess.check_output(['xdotool', 'getwindowgeometry', '--shell', window],
                        env=env, text=True).split('X=', 1)[1] for name, window in names().items()}
            with log_path.open('wb') as log:
                def launch(binary, title):
                    marker = base/title
                    process = subprocess.Popen([str(binary), '-c', 'AtomicEditFixture', '-T', title,
                        '-e', '/bin/sh', '-c', f'echo $$ > {shlex.quote(str(marker))}; exec cat'],
                        env=env, stdout=log, stderr=log)
                    processes.append(process)
                    wait(marker.exists, 'Shell did not launch: '+title+'; '+log_path.read_text(errors='replace'))
                    return marker
                try:
                    first = launch(executable, 'Kept')
                    closed = launch(executable, 'Closed')
                    wait(lambda: set(names()) == {'Kept', 'Closed'}, 'Initial windows did not settle')
                    owner = pid(names()['Kept'])
                    healthy, gate = gate_candidate(base, executable)
                    # Both edits occur after the original snapshot. The old optimistic equality
                    # check aborted here forever; the new owner must adopt the final membership.
                    added = launch(healthy, 'Added')
                    wait(lambda: set(names()) == {'Kept', 'Closed', 'Added'}, 'New window did not join the donor')
                    close_window(env, 'Closed')
                    wait(lambda: set(names()) == {'Kept', 'Added'}, 'Window close was not applied during warm-up')
                    page = base/'new-browser.html'
                    page.write_text('<!doctype html><title>AddedBrowser</title>')
                    subprocess.run([str(healthy), '-c', 'AtomicEditFixture', '--browser', str(page)],
                                   env=env, stdout=log, stderr=log, timeout=5, check=True)
                    wait(lambda: set(names()) == {'Kept', 'Added', 'AddedBrowser'},
                         'A first browser opened during preparation did not join the donor')

                    for index, window in enumerate(names().values()):
                        subprocess.run(['xdotool', 'windowmove', window, str(90+index*70), str(110+index*50),
                                        'windowsize', window, str(620+index*60), str(390+index*40)],
                                       env=env, check=True)
                    time.sleep(.2)
                    expected_geometry = geometry()
                    shell_pids = [marker.read_text().strip() for marker in (first, closed, added)]
                    gate.touch()
                    wait(lambda: processes[0].poll() is not None
                         and set(names()) == {'Kept', 'Added', 'AddedBrowser'} and all(
                        (current := pid(window)) and current != owner for window in names().values()),
                        lambda: f'Concurrent edits: owner={processes[0].poll()}, windows={names()}; '
                                + log_path.read_text(errors='replace'))
                    self.assertEqual(geometry(), expected_geometry)
                    for marker, shell in zip((first, closed, added), shell_pids):
                        os.kill(int(shell), 0)
                        self.assertEqual(marker.read_text().strip(), shell, 'Handoff restarted a live shell')
                finally:
                    (base/'continue-update').touch()
                    stop_fixture(env)
                    for process in processes:
                        process.wait(timeout=5)

    def test_input_held_by_a_master_link_survives_window_replacement(self):
        """The local daemon, not the window, holds input bound for a master: replacing the window while
        the master's transport holds it neither waits for nor loses nor repeats it."""
        with isolated_display() as display, profile(display) as env, \
                tempfile.TemporaryDirectory(prefix='replacement-input-', dir=CHECKS) as directory:
            base = Path(directory)
            executable = copy_runtime(base, browser=True)
            gate, waiting = base/'release-input', base/'input-waiting'
            env, remote = gated_master(base, env)
            ready, received = base/'ready', base/'received'
            payload = 'handoff-input'
            script = ('import os,tty\nfrom pathlib import Path\n'
                      'tty.setraw(0)\n'
                      f'Path({str(ready)!r}).write_text(str(os.getpid()))\n'
                      f'path=Path({str(received)!r})\ndata=bytearray()\n'
                      f'while len(data)<{len(payload)}:\n    data.extend(os.read(0,{len(payload)}-len(data)))\n'
                      'path.write_bytes(data)\n'
                      'while True:\n    chunk=os.read(0,65536)\n    if not chunk: break\n'
                      '    with path.open("ab") as output: output.write(chunk)\n')
            log_path = CHECKS/'window-replacement-input.log'
            with service(remote), log_path.open('wb') as log:
                owner = subprocess.Popen([str(executable), '--master', 'queued-master', '-c', 'QueuedInputFixture',
                    '-T', 'InputFence', '-e', '/usr/bin/python3', '-c', script], env=env, stdout=log, stderr=log)
                try:
                    wait(lambda: ready.exists() and normal_windows(env, 'QueuedInputFixture'),
                         lambda: 'Remote reader did not start: '+log_path.read_text(errors='replace'))
                    shell_pid = ready.read_text().strip()
                    window = normal_windows(env, 'QueuedInputFixture')[0]
                    subprocess.run(['xdotool', 'windowfocus', '--sync', window, 'type', '--clearmodifiers',
                                    '--delay', '0', payload], env=env, check=True)
                    wait(waiting.exists, 'Private transport did not hold the accepted input')
                    module = module_path(RUST_FRONTEND_MODULE, prefix=base)
                    pending = module.with_suffix('.pending')
                    shutil.copy2(module, pending)
                    pending.replace(module)
                    wait(lambda: owner.poll() is not None,
                         lambda: 'Donor waited for the master transport: '+log_path.read_text(errors='replace'))
                    self.assertFalse(received.exists())
                    gate.touch()
                    wait(received.exists,
                         lambda: 'Held input never reached the master: '+log_path.read_text(errors='replace'))
                    time.sleep(.2)
                    self.assertEqual(received.read_bytes(), payload.encode())
                    os.kill(int(shell_pid), 0)
                    self.assertEqual(ready.read_text().strip(), shell_pid)
                finally:
                    gate.touch()
                    stop_fixture(env)
                    owner.wait(timeout=5)
                    stop_fixture(remote)

    def test_terminal_windows_replace_without_restarting_shells(self):
        self.terminal_replacement(retile=False)

    def test_tiling_resize_during_activation_does_not_cancel_replacement(self):
        self.terminal_replacement(retile=True)

    def terminal_replacement(self, *, retile):
        with isolated_display() as display, profile(display) as env, service(env) as daemon, \
                tempfile.TemporaryDirectory(prefix='replacement-', dir=CHECKS) as directory:
            executable = copy_runtime(directory, browser=True)
            env = {**env, 'GDK_BACKEND': 'x11', 'GIO_USE_VFS': 'local'}
            env.pop('DBUS_SESSION_BUS_ADDRESS', None)
            log_path = Path(directory) / 'owner.log'
            processes = []
            tiler = ExitStack()
            def windows():
                result = subprocess.run(['xdotool', 'search', '--onlyvisible', '--class', 'ReplacementFixture'],
                                        env=env, capture_output=True, text=True)
                return result.stdout.splitlines() if result.returncode == 0 else []
            def pid(window):
                result = subprocess.run(['xdotool', 'getwindowpid', window], env=env,capture_output=True,text=True)
                return result.stdout.strip() if result.returncode == 0 else None
            try:
                with log_path.open('wb') as log:
                    for index in range(2):
                        marker = Path(directory) / f'shell-{index}'
                        process = subprocess.Popen([str(executable), '-c', 'ReplacementFixture', '-T', f'Window {index}',
                            '-e', '/bin/sh', '-c', f'echo $$ > {marker}; exec cat'], env=env, stdout=log, stderr=log)
                        processes.append(process)
                        wait(lambda: marker.exists() and len(windows()) == index + 1, log_path.read_text(errors='replace'))
                    owner = pid(windows()[0])
                    geometry = {}
                    for index, window in enumerate(windows()):
                        subprocess.run(['xdotool', 'windowmove', window, str(40+index*60), str(50+index*60),
                                        'windowsize', window, str(500+index*40), str(300+index*30)], env=env, check=True)
                    time.sleep(.2)
                    def state():
                        return {subprocess.check_output(['xdotool', 'getwindowname', window], env=env, text=True).strip():
                                subprocess.check_output(['xdotool', 'getwindowgeometry', '--shell', window], env=env, text=True)
                                .split('X=', 1)[1] for window in windows()}
                    geometry = state()
                    original = executable.read_bytes()
                    bad = executable.with_suffix('.pending')
                    bad.write_text('#!/bin/sh\nexit 42\n')
                    bad.chmod(0o755)
                    bad.replace(executable)
                    time.sleep(2)
                    self.assertEqual({pid(window) for window in windows()}, {owner}, 'Failed candidate closed old windows')
                    if retile:
                        resized = tiler.enter_context(retile_on_map(env, windows()))
                    bad.write_bytes(original)
                    bad.chmod(0o755)
                    bad.replace(executable)
                    shell_pids = [(Path(directory) / f'shell-{i}').read_text().strip() for i in range(2)]
                    pending_daemon = daemon.with_suffix('.pending')
                    shutil.copy2(daemon, pending_daemon)
                    pending_daemon.replace(daemon)
                    module = module_path(RUST_FRONTEND_MODULE, prefix=Path(directory))
                    replacement = module.with_suffix('.pending')
                    shutil.copy2(module, replacement)
                    replacement.replace(module)
                    wait(lambda: len(windows()) == 2 and all(
                            (current := pid(window)) and current != owner for window in windows()),
                         'Owner did not replace: ' + log_path.read_text(errors='replace'))
                    if retile:
                        self.assertTrue(resized.exists(), 'Fixture did not resize the old owner')
                        tiler.close()
                    else:
                        # A replacement owner must itself remain able to drain and replace again.
                        second_owner = pid(windows()[0])
                        shutil.copy2(module, replacement)
                        replacement.replace(module)
                        wait(lambda: len(windows()) == 2 and all(
                                (current := pid(window)) and current != second_owner for window in windows()),
                             lambda: 'Second owner did not replace: ' + log_path.read_text(errors='replace'))
                        self.assertEqual(geometry, state(), 'Window positions, sizes or selected tab titles changed')
                    for shell in shell_pids:
                        os.kill(int(shell), 0)
                    self.assertEqual(shell_pids, [(Path(directory) / f'shell-{i}').read_text().strip() for i in range(2)])
            finally:
                tiler.close()
                for identity in {value for window in windows() if (value := pid(window))}:
                    os.kill(int(identity), 15)
                for process in processes:
                    if process.poll() is None:
                        process.terminate()
                    process.wait(timeout=5)
                stop_fixture(env)


if __name__ == '__main__':
    unittest.main()
