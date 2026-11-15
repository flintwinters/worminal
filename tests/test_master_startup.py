"""Exercise the shipped launcher/module boundary through an isolated SSH bridge."""
from runtime import CHECKS

from runtime import runtime_binary


import os
from pathlib import Path
import select
import signal
import subprocess
import time
import unittest

from x11 import isolated_display


ROOT = Path(__file__).resolve().parent.parent


class MasterStartupTest(unittest.TestCase):
    def test_master_launcher_accepts_keyboard_input(self):
        executable = Path(os.environ.get("WORMINAL_TEST_EXECUTABLE", runtime_binary("worminal")))
        with isolated_display() as env:
            if master := env.get("WORMINAL_TEST_REAL_MASTER"):
                self.addCleanup(subprocess.run,
                    ["/usr/bin/ssh", "-T", "-oBatchMode=yes", "-oConnectTimeout=5", master,
                     f"env WORMINAL_SHARED_SOCKET_SCOPE={env['WORMINAL_SHARED_SOCKET_SCOPE']} worminald --stop"],
                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=15)
            env.update(GDK_BACKEND="x11", NO_AT_BRIDGE="1", GTK_USE_PORTAL="0",
                       WORMINAL_TEST_DAEMON=env.get("WORMINAL_TEST_DAEMON", str(runtime_binary("worminald"))),
                       PATH=str(ROOT / "tests/fixtures/preview-master") + os.pathsep + env["PATH"])
            env.pop("DBUS_SESSION_BUS_ADDRESS", None)
            log_path = CHECKS / "master-startup.log"
            with log_path.open("wb") as log:
                process = subprocess.Popen(
                    [str(executable), "--master", "preview-test", "-T", "Master startup check",
                     "-o", "-", "-e", "/bin/sh", "-c",
                     "stty raw -echo; printf MASTER_READY; "
                     "dd bs=1 count=1 2>/dev/null; printf MASTER_DONE"],
                    env=env, stdout=subprocess.PIPE, stderr=log, start_new_session=True)
                output = b""

                def expect(marker):
                    nonlocal output
                    deadline = time.monotonic() + 10
                    while marker not in output and time.monotonic() < deadline:
                        ready, _, _ = select.select([process.stdout], [], [],
                                                    max(0, deadline - time.monotonic()))
                        if ready:
                            chunk = os.read(process.stdout.fileno(), 4096)
                            if not chunk:
                                break
                            output += chunk
                    self.assertIn(marker, output, f"See {log_path}; status={process.poll()}")

                try:
                    expect(b"MASTER_READY")
                    window = subprocess.check_output(
                        ["xdotool", "search", "--sync", "--onlyvisible", "--pid", str(process.pid)],
                        env=env, text=True, timeout=10).splitlines()[0]
                    subprocess.run(["xdotool", "windowfocus", "--sync", window,
                                    "key", "--clearmodifiers", "x"],
                                   env=env, check=True, timeout=5, stdout=log, stderr=log)
                    expect(b"xMASTER_DONE")
                    self.assertEqual(process.wait(timeout=10), 0)
                finally:
                    # Reap every helper even if startup hangs before a window exists.
                    try:
                        os.killpg(process.pid, signal.SIGTERM)
                    except ProcessLookupError:
                        pass
                    try:
                        process.communicate(timeout=5)
                    except subprocess.TimeoutExpired:
                        os.killpg(process.pid, signal.SIGKILL)
                        process.communicate(timeout=5)

    def test_password_prompt_cancel_and_retry_in_existing_window(self):
        from tempfile import TemporaryDirectory
        from preview import profile
        from workspace_protocol import stop_workspace
        from runtime import window

        if os.environ.get('WORMINAL_TEST_REAL_MASTER'):
            self.skipTest('Authentication fixture uses a local private daemon')
        executable = Path(os.environ.get('WORMINAL_TEST_EXECUTABLE', runtime_binary('worminal')))
        with isolated_display() as display, profile(display) as env, \
                TemporaryDirectory(prefix='master-password-', dir=CHECKS) as directory:
            base = Path(directory)
            attempts, accepted = base/'attempts', base/'accepted'
            daemon = str(runtime_binary('worminald'))
            ssh = base/'ssh'
            ssh.write_text('#!/usr/bin/python3\nimport os,sys,subprocess\n'
                'from pathlib import Path\n'
                # upgrade-remote is independent of the view authentication under test.
                'if sys.argv[-1] not in ("--bridge","--bridge-existing"): sys.exit(1)\n'
                f'with open({str(attempts)!r},"a") as f: f.write("attempt\\n")\n'
                'if os.environ.get("SSH_ASKPASS_REQUIRE")!="force": sys.exit(255)\n'
                'reply=subprocess.run([os.environ["SSH_ASKPASS"],"fixture password:"],capture_output=True)\n'
                'if reply.returncode or reply.stdout!=b"fixture-secret\\n": sys.exit(255)\n'
                f'Path({str(accepted)!r}).touch()\n'
                f'os.execv({daemon!r},[{daemon!r},"--bridge"])\n')
            ssh.chmod(0o700)
            env.update(GDK_BACKEND='x11', PATH=str(base)+os.pathsep+env['PATH'])
            env.pop('DBUS_SESSION_BUS_ADDRESS', None)
            log_path = CHECKS/'master-password.log'
            with log_path.open('wb') as log:
                app = subprocess.Popen([str(executable), '--master', 'password-fixture',
                                        '-T', 'Password fixture', '-e', '/bin/cat'],
                                       env=env, stdout=log, stderr=log)
                def wait(predicate, description):
                    deadline = time.monotonic()+10
                    while time.monotonic()<deadline:
                        result = predicate()
                        if result: return result
                        self.assertIsNone(app.poll(), log_path.read_text(errors='replace'))
                        time.sleep(.03)
                    self.fail(f'Timed out: {description}; see {log_path}')
                def prompts():
                    result = subprocess.run(['xdotool','search','--onlyvisible','--name',
                                             '^Worminal SSH authentication$'],
                                            env=env,capture_output=True,text=True)
                    return result.stdout.splitlines()
                def key(wid, *keys):
                    subprocess.run(['xdotool','windowfocus','--sync',wid,'key','--clearmodifiers',*keys],
                                   env=env,check=True,stdout=log,stderr=log)
                try:
                    prompt = wait(prompts, 'initial password dialog')[0]
                    key(prompt, 'Escape')
                    wait(lambda: attempts.exists() and len(attempts.read_text().splitlines())>=2,
                         'background retry')
                    time.sleep(2)
                    self.assertFalse(prompts(), 'Cancelled authentication reopened a password dialog')
                    self.assertLessEqual(len(attempts.read_text().splitlines()), 5,
                                         'Failed SSH connections retry without backoff')
                    self.assertFalse(accepted.exists())
                    owner_window = window(env, app)
                    retry = subprocess.run([str(executable), '--master', 'password-fixture',
                                            '-e', '/bin/cat'], env=env,stdout=log,stderr=log,timeout=10)
                    self.assertEqual(retry.returncode,0)
                    prompt = wait(prompts, 'explicit password retry')[0]
                    subprocess.run(['xdotool','windowfocus','--sync',prompt,'type','--clearmodifiers',
                                    'fixture-secret'],env=env,check=True,stdout=log,stderr=log)
                    key(prompt, 'Return')
                    wait(accepted.exists, 'password delivered privately to SSH')
                    wait(lambda: not prompts(), 'password dialog closes')
                    self.assertEqual(window(env,app),owner_window,'Authentication replaced the window')
                    self.assertNotIn('fixture-secret',log_path.read_text(errors='replace'))
                finally:
                    app.terminate()
                    app.wait(timeout=5)
                    stop_workspace(env)
