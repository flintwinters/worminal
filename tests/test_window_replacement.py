"""Check automatic owner replacement on a private X server with a copied installation."""
import os
from pathlib import Path
import shutil
import signal
import select
import subprocess
import tempfile
import time
import unittest
from preview import profile, service, fixture, Fixture
from runtime import CHECKS, copy_runtime, module_path, runtime_binary, RUST_FRONTEND_MODULE, RUST_BROWSER_MODULE
from x11 import isolated_display


def wait(check, message):
    deadline = time.monotonic() + 25
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(.03)
    raise AssertionError(message)


def stop_fixture(env):
    """Wait for this private scope's owners and browser workers before deleting mapped files."""
    identities = []
    scope = ('WORMINAL_SHARED_SOCKET_SCOPE='+env['WORMINAL_SHARED_SOCKET_SCOPE']).encode()
    for process in Path('/proc').iterdir():
        if not process.name.isdecimal() or int(process.name) == os.getpid(): continue
        try:
            identity = os.pidfd_open(int(process.name))
            if scope not in (process/'environ').read_bytes().split(b'\0'):
                os.close(identity)
                continue
            identities.append(identity)
            signal.pidfd_send_signal(identity, signal.SIGTERM)
        except (ProcessLookupError, FileNotFoundError, PermissionError):
            continue
    for identity in identities:
        try:
            poll = select.poll()
            poll.register(identity, select.POLLIN)
            if not poll.poll(5000):
                signal.pidfd_send_signal(identity, signal.SIGKILL)
                if not poll.poll(5000): raise AssertionError('Private fixture process did not exit')
        finally:
            os.close(identity)


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
                    healthy = Path(directory) / 'healthy-browser.so'
                    shutil.copy2(browser, healthy)
                    marker = Path(directory) / 'import-attempted'
                    shim = Path(directory) / 'fail-import.c'
                    shim.write_text('#include <dlfcn.h>\n#include <stdio.h>\n#include "rust_browser_module.h"\n'
                        f'static void *library(void) {{ static void *h; if(!h) h=dlopen("{healthy}",RTLD_NOW|RTLD_LOCAL); return h; }}\n'
                        'const WorminalRustBrowser *worminal_rust_browser_module(uint32_t v) {'
                        'const WorminalRustBrowser *(*entry)(uint32_t)=dlsym(library(),"worminal_rust_browser_module");'
                        'return entry(v); }\n'
                        'int32_t worminal_browser_profile_transfer_v1(void *p,const uint8_t *b,uint64_t n,'
                        'WorminalBrowserProfileReply reply,void *ctx) {'
                        f'if(b){{FILE *f=fopen("{marker}","w");if(f)fclose(f);reply(ctx,1,0,0);return 1;}}'
                        'WorminalBrowserProfileTransfer transfer=dlsym(library(),"worminal_browser_profile_transfer_v1");'
                        'return transfer(p,b,n,reply,ctx); }\n')
                    pending_browser = browser.with_suffix('.pending')
                    subprocess.run(['cc', '-shared', '-fPIC', '-I', str(Path(__file__).resolve().parent.parent/'src/gtk'),
                                    str(shim), '-ldl', '-o', str(pending_browser)], check=True)
                    pending_browser.replace(browser)
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

    def test_multihost_links_and_selected_remote_tab_survive_replacement(self):
        with isolated_display() as display, profile(display) as env, service(env), \
                tempfile.TemporaryDirectory(prefix='replacement-links-', dir=CHECKS) as directory:
            executable = copy_runtime(directory, browser=True)
            base = Path(directory)
            scopes = {host: env['WORMINAL_SHARED_SOCKET_SCOPE']+'-'+host for host in ('remotea', 'remoteb')}
            ssh = base/'ssh'
            daemon = str(runtime_binary('worminald'))
            ssh.write_text('#!/usr/bin/python3\nimport os,sys\n'
                'assert sys.argv[1:4]==["-T","-C","-oConnectTimeout=10"]\n'
                f'scopes={scopes!r}\n'
                'assert sys.argv[4] in scopes and sys.argv[-1] in ("--bridge","--bridge-existing")\n'
                'os.environ["WORMINAL_SHARED_SOCKET_SCOPE"]=scopes[sys.argv[4]]\n'
                f'os.execv({daemon!r},[{daemon!r},sys.argv[-1]])\n')
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
                # A listed window can be destroyed by the replacement before its pid is read.
                found = subprocess.run(['xdotool','getwindowpid',windows[0]],env=env,capture_output=True,text=True) if windows else None
                return found.stdout.strip() if found and found.returncode==0 else None
            with service(remotea), service(remoteb), (base/'owner.log').open('wb') as log:
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
                    pending = module_path(RUST_FRONTEND_MODULE,prefix=base).with_suffix('.pending')
                    shutil.copy2(module_path(RUST_FRONTEND_MODULE,prefix=base),pending)
                    pending.replace(module_path(RUST_FRONTEND_MODULE,prefix=base))
                    wait(lambda: owner() and owner()!=previous,'Multi-host window did not replace')
                    self.assertEqual(len(visible()),1)
                    name = subprocess.check_output(['xdotool','getwindowname',visible()[0]],env=env,text=True).strip()
                    self.assertEqual(name,'remoteb','Selected remote tab was not restored')
                    for host, pid in identities.items():
                        os.kill(int(pid),0)
                        self.assertEqual((base/host).read_text().strip(),pid,'Update replayed remote launch')
                finally:
                    stop_fixture(env)
                    if process is not None: process.wait(timeout=5)

    def test_terminal_windows_replace_without_restarting_shells(self):
        with isolated_display() as display, profile(display) as env, service(env), \
                tempfile.TemporaryDirectory(prefix='replacement-', dir=CHECKS) as directory:
            executable = copy_runtime(directory, browser=True)
            env = {**env, 'GDK_BACKEND': 'x11', 'GIO_USE_VFS': 'local'}
            env.pop('DBUS_SESSION_BUS_ADDRESS', None)
            log_path = Path(directory) / 'owner.log'
            processes = []
            def windows():
                result = subprocess.run(['xdotool', 'search', '--onlyvisible', '--class', 'ReplacementFixture'],
                                        env=env, capture_output=True, text=True)
                return result.stdout.splitlines() if result.returncode == 0 else []
            def pid(window):
                return subprocess.check_output(['xdotool', 'getwindowpid', window], env=env, text=True).strip()
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
                    bad.write_bytes(original)
                    bad.chmod(0o755)
                    bad.replace(executable)
                    shell_pids = [(Path(directory) / f'shell-{i}').read_text().strip() for i in range(2)]
                    module = module_path(RUST_FRONTEND_MODULE, prefix=Path(directory))
                    replacement = module.with_suffix('.pending')
                    shutil.copy2(module, replacement)
                    replacement.replace(module)
                    wait(lambda: len(windows()) == 2 and all(pid(window) != owner for window in windows()),
                         'Owner did not replace: ' + log_path.read_text(errors='replace'))
                    self.assertEqual(geometry, state(), 'Window positions, sizes or selected tab titles changed')
                    for shell in shell_pids:
                        os.kill(int(shell), 0)
                    self.assertEqual(shell_pids, [(Path(directory) / f'shell-{i}').read_text().strip() for i in range(2)])
            finally:
                for identity in {pid(window) for window in windows()}:
                    os.kill(int(identity), 15)
                for process in processes:
                    if process.poll() is None:
                        process.terminate()
                    process.wait(timeout=5)
                stop_fixture(env)


if __name__ == '__main__':
    unittest.main()
