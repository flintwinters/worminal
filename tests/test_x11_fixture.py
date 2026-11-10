"""Display diagnostics must not block a private X server before it reports readiness."""
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from runtime import CHECKS
import x11


class X11FixtureTest(unittest.TestCase):
    def test_large_server_diagnostics_cannot_fill_a_pipe_and_hang_startup(self):
        with tempfile.TemporaryDirectory(prefix='xvfb-fixture-', dir=CHECKS) as directory:
            root = Path(directory)
            executable = root/'Xvfb'
            executable.write_text('#!/usr/bin/python3\nimport os,sys,signal\n'
                f'open({str(root/"pid")!r},"w").write(str(os.getpid()))\n'
                'os.write(2,b"fixture diagnostic\\n"*32768)\n'
                'os.write(int(sys.argv[sys.argv.index("-displayfd")+1]),b"77\\n")\n'
                'while True:signal.pause()\n')
            executable.chmod(0o700)
            (root/'xdotool').write_text('#!/bin/sh\nexit 0\n')
            (root/'xdotool').chmod(0o700)
            with patch.dict(os.environ, {'PATH': str(root)+os.pathsep+os.environ['PATH']}), patch.object(x11,'CHECKS',root):
                with x11.isolated_display() as env:
                    self.assertEqual(env['DISPLAY'], ':77')
                    pid = int((root/'pid').read_text())
                self.assertFalse(Path(f'/proc/{pid}').exists())
            logs = list(root.glob('xvfb-*.log'))
            self.assertEqual(len(logs),1)
            self.assertGreater(logs[0].stat().st_size, 65536)


if __name__ == '__main__':
    unittest.main()
