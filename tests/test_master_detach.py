"""Exercise master-scoped frontend commands on private displays and real private daemons."""
import json
import os
import shutil
from pathlib import Path
import subprocess
import tempfile
import time
import unittest

from runtime import CHECKS, runtime_binary, copy_runtime, module_path, RUST_FRONTEND_MODULE
from preview import profile
from test_window_state import private_bus, OBJECT, INTERFACE
from workspace_protocol import stop_workspace
from x11 import isolated_display

ROOT = Path(__file__).resolve().parent.parent


class MasterDetachTest(unittest.TestCase):
    def wait(self, predicate, description):
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if result := predicate():
                return result
            time.sleep(.03)
        self.fail(f'Timed out: {description}')

    def test_detach_preserves_shells_other_hosts_and_reconnects(self):
        with isolated_display() as display, private_bus(display) as session, profile(session) as env, \
                tempfile.TemporaryDirectory(prefix='master-detach-hosts-', dir=CHECKS) as directory:
            base = Path(directory)
            executable = str(copy_runtime(base, browser=True))
            scopes = {host: env['WORMINAL_SHARED_SOCKET_SCOPE']+'-'+host
                      for host in ('preview-test', 'wheel-test')}
            ssh = base/'ssh'
            ssh.write_text('#!/usr/bin/python3\nimport os,sys,time\n'
                           f'scopes={scopes!r}\n'
                           f'if sys.argv[4]=="preview-test" and os.path.exists({str(base/"delay-primary")!r}): time.sleep(1)\n'
                           'os.environ["WORMINAL_SHARED_SOCKET_SCOPE"]=scopes[sys.argv[4]]\n'
                           f'sys.path.insert(0,{str(ROOT/"tests/fixtures")!r})\n'
                           'from master_ssh import run\n'
                           f'run({str(runtime_binary("worminald"))!r})\n')
            ssh.chmod(0o700)
            env.update(GDK_BACKEND='x11', SHELL='/bin/sh', PATH=str(base)+os.pathsep+env['PATH'])
            with (CHECKS/'master-detach.log').open('wb') as log:
                app = subprocess.Popen([executable, '-c', 'DetachFixture', '--master', 'preview-test', '-e', '/bin/cat'],
                                       env=env, stdout=log, stderr=log)
                def cli(*args, success=True):
                    result = subprocess.run([executable, *args], env=env, capture_output=True, timeout=10)
                    self.assertEqual(result.returncode == 0, success, result.stderr)
                    return result
                def owner():
                    result = subprocess.run(['xdotool', 'search', '--onlyvisible', '--class', 'DetachFixture'],
                                            env=env, capture_output=True, text=True, timeout=3)
                    if not result.stdout.strip():
                        return None
                    pid = subprocess.run(['xdotool', 'getwindowpid', result.stdout.splitlines()[0]],
                                         env=env, capture_output=True, text=True, timeout=3)
                    return pid.stdout.strip() if pid.returncode == 0 else None
                def windows():
                    pid = owner()
                    if not pid:
                        return []
                    result = subprocess.run(['busctl', '--user', '--json=short', 'call',
                        f'org.worminal.Instance.p{pid}', OBJECT, INTERFACE, 'GetWindows'],
                        env=env, capture_output=True, text=True, timeout=3)
                    return json.loads(result.stdout)['data'][0] if result.returncode == 0 else []
                remote_env = {**env, 'WORMINAL_SHARED_SOCKET_SCOPE': scopes['preview-test']}
                def tabs():
                    result = subprocess.run([str(runtime_binary('worminald')), 'api', 'list'],
                                            env=remote_env, capture_output=True, text=True, check=True, timeout=5)
                    # View ownership changes by design; compare persistent tab and shell identity.
                    fields = ('id', 'kind', 'shell_pid', 'title', 'directory', 'url')
                    return {'tabs': [{key: tab[key] for key in fields}
                                     for tab in json.loads(result.stdout)['tabs']]}
                try:
                    self.wait(lambda: windows() and windows()[0][2] and windows()[0][4] == 'preview-test', 'primary master')
                    subprocess.run([str(runtime_binary('worminald')), 'api', 'open', '--', '/bin/cat'],
                                   env=remote_env, check=True, capture_output=True, timeout=5)
                    self.wait(lambda: len(tabs()['tabs']) == 2, 'multiple remote tabs')
                    before = tabs()
                    cli('--master', 'wheel-test', '-e', '/bin/cat')
                    self.wait(lambda: windows()[0][4] == 'wheel-test', 'second master selected')
                    cli('--attach', '-c', 'DetachEmpty', '-T', 'DetachEmpty')
                    self.wait(lambda: len(windows()) == 2, 'second window')
                    empty = subprocess.check_output(['xdotool', 'search', '--onlyvisible', '--name',
                                                     '^DetachEmpty$'], env=env, text=True).splitlines()[0]
                    subprocess.run(['xdotool', 'windowfocus', '--sync', empty], env=env, check=True, timeout=3)
                    cli('--master', 'preview-test', '--attach')
                    self.wait(lambda: sum(row[4] == 'preview-test' for row in windows()) == 1,
                              'master attached to second window')
                    cli('--detach-master', 'preview-test')
                    self.wait(lambda: len(windows()) == 1, 'empty second window closes')
                    self.assertIsNone(app.poll())
                    self.assertEqual(windows()[0][4], 'wheel-test')
                    self.assertEqual(tabs(), before, 'Detach changed daemon-owned tabs')
                    # The launcher once printed only "GTK owner rejected launch".
                    self.assertIn(b'exact endpoint of one of: wheel-test',
                                  cli('--detach-master', 'preview-test', success=False).stderr)
                    previous = owner()
                    module = module_path(RUST_FRONTEND_MODULE, prefix=base)
                    pending = module.with_suffix('.pending')
                    shutil.copy2(module, pending)
                    pending.replace(module)
                    self.wait(lambda: owner() and owner() != previous, 'owner replacement after detach')
                    self.wait(lambda: windows() and windows()[0][4] == 'wheel-test', 'remaining host restored')
                    cli('--detach-master', 'preview-test', success=False)
                    self.assertEqual(tabs(), before, 'Replacement changed detached shells')
                    (base/'delay-primary').touch()
                    cli('--master', 'preview-test', '--attach')
                    cli('--detach-master', 'wheel-test')
                    self.wait(lambda: windows() and windows()[0][2] and windows()[0][4] == 'preview-test',
                              'explicit reattachment survives detaching the other host while pending')
                    cli('--detach-master', 'preview-test')
                    self.wait(lambda: not owner(), 'empty replacement window closes')
                    self.assertEqual(app.wait(timeout=5), 0)
                    self.assertEqual(tabs(), before, 'Last window close ended remote shells')
                    cli('--detach-master', 'preview-test', success=False)
                finally:
                    from test_window_replacement import stop_fixture
                    stop_fixture(env)
                    app.wait(timeout=5)
                    stop_workspace(env)
                    for scope in scopes.values():
                        stop_workspace({**env, 'WORMINAL_SHARED_SOCKET_SCOPE': scope})

    def test_detach_cancels_pending_reconnect(self):
        executable = str(runtime_binary('worminal'))
        with isolated_display() as display, profile(display) as env, \
                tempfile.TemporaryDirectory(prefix='master-detach-', dir=CHECKS) as directory:
            base = Path(directory)
            attempts = base/'attempts'
            ssh = base/'ssh'
            ssh.write_text('#!/usr/bin/python3\n'
                           f'with open({str(attempts)!r}, "a") as f: f.write("attempt\\n")\n'
                           'raise SystemExit(255)\n')
            ssh.chmod(0o700)
            env.update(GDK_BACKEND='x11', SHELL='/bin/sh', PATH=str(base)+os.pathsep+env['PATH'])
            with (base/'frontend.log').open('wb') as log:
                app = subprocess.Popen([executable, '-e', '/bin/cat'], env=env, stdout=log, stderr=log)
                try:
                    from runtime import window
                    window(env, app)
                    subprocess.run([executable, '--master', 'failed-master', '--attach'],
                                   env=env, check=True, stdout=log, stderr=log, timeout=10)
                    self.wait(lambda: attempts.exists() and len(attempts.read_text().splitlines()) >= 2,
                              'background retry')
                    subprocess.run([executable, '--detach-master', 'failed-master'],
                                   env=env, check=True, stdout=log, stderr=log, timeout=10)
                    # A retry would fire within one second at this point in the backoff sequence.
                    time.sleep(.2)
                    before = attempts.read_text()
                    time.sleep(2)
                    self.assertEqual(attempts.read_text(), before, 'Detached master retried SSH')
                    self.assertIsNone(app.poll(), 'Unrelated local window closed')
                finally:
                    app.terminate()
                    app.wait(timeout=5)
                    stop_workspace(env)

    def test_close_master_ends_only_target_tabs_across_windows(self):
        with isolated_display() as display, profile(display) as env, \
                tempfile.TemporaryDirectory(prefix='master-close-', dir=CHECKS) as directory:
            base = Path(directory)
            executable = str(copy_runtime(base, browser=True))
            scopes = {host: env['WORMINAL_SHARED_SOCKET_SCOPE']+'-'+host
                      for host in ('target-master', 'other-master')}
            ssh = base/'ssh'
            ssh.write_text('#!/usr/bin/python3\nimport os,sys\n'
                           f'os.environ["WORMINAL_SHARED_SOCKET_SCOPE"]={scopes!r}[sys.argv[4]]\n'
                           f'sys.path.insert(0,{str(ROOT/"tests/fixtures")!r})\n'
                           'from master_ssh import run\n'
                           f'run({str(runtime_binary("worminald"))!r})\n')
            ssh.chmod(0o700)
            env.update(GDK_BACKEND='x11', SHELL='/bin/sh', PATH=str(base)+os.pathsep+env['PATH'])
            def cli(*arguments, success=True):
                result = subprocess.run([executable, *arguments], env=env,
                                        capture_output=True, timeout=10)
                self.assertEqual(result.returncode == 0, success, result.stderr)
            def api(host, *arguments):
                target = {**env, 'WORMINAL_SHARED_SOCKET_SCOPE': scopes.get(host, env['WORMINAL_SHARED_SOCKET_SCOPE'])}
                result = subprocess.run([str(runtime_binary('worminald')), 'api', *arguments],
                                        env=target, capture_output=True, text=True, check=True, timeout=5)
                return json.loads(result.stdout)
            def ready_tabs(host):
                try:
                    return api(host, 'list')['tabs']
                except subprocess.CalledProcessError:
                    return None  # The asynchronous SSH bootstrap has not created its private daemon yet.
            with (CHECKS/'master-close.log').open('wb') as log:
                app = subprocess.Popen([executable, '-c', 'CloseFixture', '--master', 'target-master', '-e', '/bin/cat'],
                                       env=env, stdout=log, stderr=log)
                try:
                    from runtime import window
                    window(env, app)
                    self.wait(lambda: ready_tabs('target-master'), 'target tab')
                    api('target-master', 'open', '--', '/bin/cat')
                    api('target-master', 'open', '--browser', 'about:blank')
                    before = api('target-master', 'list')['tabs']
                    shells = [tab['shell_pid'] for tab in before if tab['shell_pid']]
                    self.assertEqual(len(before), 3)
                    cli('--master', 'other-master', '-e', '/bin/cat')
                    other = self.wait(lambda: ready_tabs('other-master'), 'other master')
                    cli('-c', 'CloseMirror', '-T', 'CloseMirror', '-e', '/bin/cat')
                    local = self.wait(lambda: ready_tabs(''), 'local tab')
                    cli('--master', 'target-master', '--attach')
                    cli('--close-master', 'unknown-master', success=False)
                    # A wrong endpoint must not close a similarly scoped or selected host.
                    self.assertEqual(len(api('target-master', 'list')['tabs']), 3)
                    cli('--close-master', 'target-master')
                    self.wait(lambda: not api('target-master', 'list')['tabs'], 'target tabs closed')
                    self.wait(lambda: all(not Path(f'/proc/{pid}').exists() for pid in shells), 'target programs terminated')
                    self.assertEqual([tab['shell_pid'] for tab in api('other-master', 'list')['tabs']],
                                     [tab['shell_pid'] for tab in other])
                    self.assertEqual([tab['shell_pid'] for tab in api('', 'list')['tabs']],
                                     [tab['shell_pid'] for tab in local])
                    self.assertIsNone(app.poll(), 'Unrelated master window closed')
                    cli('--close-master', 'target-master', success=False)
                    cli('--detach-master', 'other-master')
                    for tab in local:
                        api('', 'close', str(tab['id']))
                    self.assertEqual(app.wait(timeout=5), 0)
                finally:
                    from test_window_replacement import stop_fixture
                    stop_fixture(env)
                    app.wait(timeout=5)
                    stop_workspace(env)
                    for scope in scopes.values():
                        stop_workspace({**env, 'WORMINAL_SHARED_SOCKET_SCOPE': scope})

    def test_invalid_command_never_opens_an_owner(self):
        with isolated_display() as env:
            for option in ('--detach-master', '--close-master'):
                for args in [(option,), (option, ''), (option, 'host', '--attach'), (option, 'host')]:
                    result = subprocess.run([str(runtime_binary('worminal')), *args], env=env,
                                            capture_output=True, timeout=5)
                    self.assertNotEqual(result.returncode, 0)
