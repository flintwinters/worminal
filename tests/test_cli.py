"""Help and version must identify the build without launching a graphical session."""
from runtime import CHECKS

from runtime import runtime_binary


import os
import errno
import json
import pty
import re
import select
import shutil
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest

from tools.build import VERSION


ROOT = Path(__file__).resolve().parent.parent


def human_environment(variables=None):
    # Start from a minimal human environment without copying the product's marker registry.
    inherited = ('PATH', 'HOME', 'USER', 'LOGNAME', 'SHELL', 'LD_LIBRARY_PATH', 'LANG', 'LC_ALL', 'GIO_USE_VFS')
    env = {name: value for name, value in os.environ.items()
           if name in inherited or name.startswith(('XDG_', 'WORMINAL_TEST_'))}
    env.update(TERM='xterm-256color', GDK_BACKEND='x11',
               WORMINAL_SHARED_SOCKET_SCOPE='color-test-absent')
    env.update(variables or {})
    return env


class CommandLineTest(unittest.TestCase):
    def test_native_help_automatically_exposes_searchable_commands_and_options(self):
        env = human_environment()
        daemon = str(runtime_binary('worminald'))
        references = [subprocess.check_output([str(runtime_binary(binary)), '--help'], env=env, text=True)
                      for binary in ('worminal', 'worminald')]
        # Discover commands from generated help, not a test-owned command/option list.
        commands = set(re.findall(r'^  worminald (\S+)', references[1], re.MULTILINE))
        for command in sorted(commands):
            result = subprocess.run([daemon, command, '--help'], env=env,
                                    capture_output=True, text=True, timeout=5)
            self.assertEqual(result.returncode, 0, (command, result.stderr))
            references.append(result.stdout)
        options = set(re.findall(r'(?<![\w-])--[a-z][a-z-]*|(?<![\w-])-[a-zA-Z](?!\w)',
                                 '\n'.join(references)))
        self.assertTrue(commands)
        self.assertTrue(options)
        for token in sorted(commands | options):
            with self.subTest(token=token):
                result = subprocess.run([daemon, 'api', 'help', 'search', token], env=env,
                                        capture_output=True, text=True, timeout=5)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn(token, result.stdout)
                self.assertNotIn('No matching API help', result.stdout)

    def terminal_output(self, binary, *arguments, variables=None, stderr=False):
        env = human_environment(variables)
        master, slave = pty.openpty()
        try:
            process = subprocess.Popen([str(runtime_binary(binary)), *arguments], env=env,
                                       stdout=subprocess.PIPE if stderr else slave,
                                       stderr=slave if stderr else subprocess.PIPE)
            os.close(slave)
            slave = None
            output = bytearray()
            deadline = time.monotonic() + 10
            try:
                while True:
                    self.assertLess(time.monotonic(), deadline, 'CLI terminal output timed out')
                    if not select.select([master], [], [], .1)[0]:
                        continue
                    try:
                        data = os.read(master, 65536)
                    except OSError as error:
                        if error.errno != errno.EIO:
                            raise
                        break
                    if not data:
                        break
                    output.extend(data)
                other, errors = process.communicate(timeout=5)
                self.assertEqual((other or b'') + (errors or b''), b'')
                return process.returncode, output.decode().replace('\r\n', '\n')
            finally:
                if process.poll() is None:
                    process.kill()
                process.wait(timeout=5)
        finally:
            os.close(master)
            if slave is not None:
                os.close(slave)

    def test_redirected_output_is_plain_without_agent_markers(self):
        for binary in ('worminal', 'worminald'):
            for arguments in [('--help',), ('--version',), ('--new-browser-tab', 'one', 'two')]:
                result = subprocess.run([str(runtime_binary(binary)), *arguments],
                                        env=human_environment(), capture_output=True, timeout=5)
                self.assertNotIn(b'\x1b[', result.stdout + result.stderr)

    def test_terminal_help_color_is_disabled_for_agents_and_color_opt_outs(self):
        for binary, arguments in [('worminal', ('--help',)), ('worminal', ('--version',)),
                                  ('worminald', ('--help',)), ('worminald', ('--version',)),
                                  ('worminald', ('api', 'help')),
                                  ('worminald', ('api', 'help', 'search', 'paste')),
                                  ('worminald', ('config', '--help')),
                                  ('worminald', ('upgrade-remote', '--help')),
                                  ('worminald', ('--list-masters', '--help'))]:
            status, colored = self.terminal_output(binary, *arguments)
            self.assertEqual(status, 0)
            self.assertIn('\x1b[', colored, binary)
            plain = re.sub(r'\x1b\[[0-9;]*m', '', colored)
            for variables in [{'CODEX_THREAD_ID': ''}, {'CLAUDECODE': '1'}, {'CURSOR_AGENT': '1'},
                              {'NO_COLOR': ''}, {'CLICOLOR': '0'}, {'TERM': 'dumb'}]:
                with self.subTest(binary=binary, arguments=arguments, variables=variables):
                    status, output = self.terminal_output(binary, *arguments, variables=variables)
                    self.assertEqual(status, 0)
                    self.assertEqual(output, plain)

    def test_terminal_json_replies_are_laid_out_for_humans_and_compact_for_agents(self):
        status, human = self.terminal_output('worminald', '--list-masters')
        self.assertEqual(status, 0)
        self.assertNotIn('{', human)
        self.assertRegex(re.sub(r'\x1b\[[0-9;]*m', '', human), r'masters\s+-')
        status, agent = self.terminal_output('worminald', '--list-masters', variables={'CLAUDECODE': '1'})
        self.assertEqual(status, 0)
        self.assertEqual(json.loads(agent)['masters'], [])

    def test_terminal_errors_use_stderr_policy_and_data_outputs_stay_plain(self):
        for binary, arguments in [('worminal', ('--new-browser-tab', 'one', 'two')),
                                  ('worminald', ('api', 'help', 'unknown-color-test'))]:
            status, colored = self.terminal_output(binary, *arguments, stderr=True)
            self.assertEqual(status, 2)
            self.assertIn('\x1b[31m', colored)
            status, plain = self.terminal_output(binary, *arguments, stderr=True, variables={'AGENT': ''})
            self.assertEqual(status, 2)
            self.assertEqual(plain, re.sub(r'\x1b\[[0-9;]*m', '', colored))
        for arguments in [('api', 'list'), ('config', '--json', '--defaults'), ('config', '--example'),
                          ('api', 'help', 'shell-bash'), ('api', 'help', 'shell-zsh')]:
            _, output = self.terminal_output('worminald', *arguments)
            self.assertNotIn('\x1b[', output)

    def test_browser_command_does_not_require_a_frontend_module(self):
        checks = CHECKS
        checks.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="browser-command-", dir=checks) as directory:
            path = Path(directory)
            for name in ("worminal", "worminald"):
                shutil.copy2(runtime_binary(name), path / name)
            env = {**os.environ, "WORMINAL_SHARED_SOCKET_SCOPE": path.name,
                   "DISPLAY": "invalid", "WAYLAND_DISPLAY": "invalid"}
            result = subprocess.run([str(path / "worminal"), "--new-browser-tab", "about:blank"],
                                    env=env, capture_output=True, text=True, timeout=5)
            self.assertEqual(result.returncode, 1)
            self.assertIn("No preview workspace service is running", result.stderr)
            invalid = subprocess.run([str(path / "worminal"), "--new-browser-tab", "one", "two"],
                                     env=env, capture_output=True, text=True, timeout=5)
            self.assertEqual(invalid.returncode, 2)
            self.assertIn("usage:", invalid.stderr)
            for name in ("worminal", "worminald"):
                # Without a target the command proceeds to the service with the blank browser home.
                default = subprocess.run([str(path / name), "--new-browser-tab"],
                                         env=env, capture_output=True, text=True, timeout=5)
                self.assertEqual(default.returncode, 1)
                self.assertIn("No preview workspace service is running", default.stderr)


    def test_launcher_does_not_link_graphics_libraries(self):
        dependencies = subprocess.check_output(["ldd", str(runtime_binary("worminal"))], text=True)
        for name in ("libgtk", "libgdk", "libpango", "libwebkit"):
            self.assertNotIn(name, dependencies)


    def invoke(self, option):
        env = {key: value for key, value in os.environ.items()
               if key not in ("DISPLAY", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS")}
        env["GDK_BACKEND"] = "x11"
        result = subprocess.run([str(runtime_binary("worminal")), option], env=env,
                                capture_output=True, text=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stderr, "")
        return result.stdout

    def test_help_without_a_display(self):
        for option in ("-h", "--help"):
            with self.subTest(option=option):
                output = self.invoke(option)
                self.assertEqual(output.splitlines()[0], f"worminal {VERSION}")
                self.assertIn("usage: worminal", output)
                self.assertIn("--help", output)
                self.assertIn("--version", output)

    def test_version_without_a_display(self):
        for option in ("-v", "--version"):
            with self.subTest(option=option):
                self.assertEqual(self.invoke(option), f"worminal {VERSION}\n")
