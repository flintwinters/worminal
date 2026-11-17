"""Check automatic owner replacement on a private X server with a copied installation."""
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
from preview import profile, service, fixture, Fixture
from runtime import ROOT, CHECKS, copy_runtime, module_path, runtime_binary, RUST_FRONTEND_MODULE, RUST_BROWSER_MODULE
from x11 import isolated_display, close_window, normal_windows


def wait(check, message):
    deadline = time.monotonic() + 25
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(.03)
    raise AssertionError(message() if callable(message) else message)


def stop_fixture(env):
    """Wait for this private scope's owners and browser workers before deleting mapped files."""
    identities = []
    scope = ('WORMINAL_SHARED_SOCKET_SCOPE='+env['WORMINAL_SHARED_SOCKET_SCOPE']).encode()
    for process in Path('/proc').iterdir():
        if not process.name.isdecimal() or int(process.name) == os.getpid(): continue
        identity = None
        try:
            identity = os.pidfd_open(int(process.name))
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
                'assert sys.argv[4] in scopes\n'
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
                    def replacement_owner():
                        current = owner()
                        return current if current and current != previous else None
                    wait(replacement_owner,'Multi-host window did not replace')
                    self.assertEqual(len(visible()),1)
                    name = subprocess.check_output(['xdotool','getwindowname',visible()[0]],env=env,text=True).strip()
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

    def test_accepted_input_is_fenced_through_a_backpressured_master(self):
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
                    wait(lambda: '-> Draining' in log_path.read_text(errors='replace'),
                         lambda: 'Donor did not wait for its input fence: '+log_path.read_text(errors='replace'))
                    self.assertIsNone(owner.poll(), 'Donor retired while accepted input was still in transport')
                    self.assertFalse(received.exists())
                    gate.touch()
                    wait(lambda: owner.poll() is not None and received.exists(),
                         lambda: 'Input fence did not complete the handoff: '+log_path.read_text(errors='replace'))
                    self.assertEqual(received.read_bytes(), payload.encode())
                    os.kill(int(shell_pid), 0)
                    self.assertEqual(ready.read_text().strip(), shell_pid)
                finally:
                    gate.touch()
                    stop_fixture(env)
                    owner.wait(timeout=5)
                    stop_fixture(remote)

    def test_drain_fence_survives_a_reconnected_master(self):
        with isolated_display() as display, profile(display) as env, \
                tempfile.TemporaryDirectory(prefix='replacement-reconnect-', dir=CHECKS) as directory:
            base = Path(directory)
            executable = copy_runtime(base, browser=True)
            env, remote = gated_master(base, env)
            dropped, ready = base/'fence-dropped', base/'shell-ready'
            env['WORMINAL_TEST_DROP_FENCE'] = str(dropped)
            log_path = CHECKS/'window-replacement-reconnect.log'
            with service(remote), log_path.open('wb') as log:
                owner = subprocess.Popen([str(executable), '--master', 'reconnect-master',
                    '-c', 'ReconnectFixture', '-T', 'Reconnect', '-e', '/bin/sh', '-c',
                    f'echo $$ > {shlex.quote(str(ready))}; exec cat'], env=env, stdout=log, stderr=log)
                try:
                    wait(lambda: ready.exists() and normal_windows(env, 'ReconnectFixture'),
                         'Remote shell did not start')
                    shell_pid = ready.read_text().strip()
                    module = module_path(RUST_FRONTEND_MODULE, prefix=base)
                    pending = module.with_suffix('.pending')
                    shutil.copy2(module, pending)
                    pending.replace(module)
                    wait(dropped.exists, 'Fixture did not drop the donor fence')
                    wait(lambda: owner.poll() is not None,
                         lambda: 'Reconnected fence stalled handoff: '+log_path.read_text(errors='replace'))
                    wait(lambda: normal_windows(env, 'ReconnectFixture'), 'Replacement window did not map')
                    self.assertEqual(owner.returncode, 0)
                    os.kill(int(shell_pid), 0)
                    self.assertEqual(ready.read_text().strip(), shell_pid, 'Handoff replayed the shell launch')
                finally:
                    stop_fixture(env)
                    owner.wait(timeout=5)
                    stop_fixture(remote)

    def test_terminal_windows_replace_without_restarting_shells(self):
        with isolated_display() as display, profile(display) as env, service(env) as daemon, \
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
                for identity in {value for window in windows() if (value := pid(window))}:
                    os.kill(int(identity), 15)
                for process in processes:
                    if process.poll() is None:
                        process.terminate()
                    process.wait(timeout=5)
                stop_fixture(env)


if __name__ == '__main__':
    unittest.main()
