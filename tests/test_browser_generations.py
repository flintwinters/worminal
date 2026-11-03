"""New tabs load updated real browser modules while older WebKit views keep their state."""
from runtime import CHECKS

import os
from pathlib import Path
import shutil
import subprocess
import unittest

from preview import profile, fixture
from x11 import isolated_display

ROOT = Path(__file__).resolve().parent.parent


class BrowserGenerationTest(unittest.TestCase):
    def test_compatible_generations_preserve_old_live_views(self):
        with isolated_display() as display, profile(display) as env, fixture() as url:
            base = Path(env['XDG_DATA_HOME']).parent
            executable = base / 'bin' / 'worminal'
            executable.parent.mkdir()
            shutil.copy2(os.environ.get('WORMINAL_TEST_BROWSER_GENERATIONS_EXECUTABLE',
                                       ROOT / 'build/browser_generations_test'), executable)
            original = (ROOT / 'build/browser.so').read_bytes()
            marker = b'\0URL\0'
            self.assertEqual(original.count(marker), 1, 'location-entry version marker must be unique')
            updated = base / 'updated.so'
            updated.write_bytes(original.replace(marker, b'\0NEW\0'))
            installed = base / 'lib' / 'worminal'
            installed.mkdir(parents=True)
            # Use the build's ABI-named install destination, not a separate test naming decision.
            import sys
            sys.path.insert(0, str(ROOT))
            from tools.reference import module
            BROWSER_MODULE = module().BROWSER_MODULE
            shutil.copy2(ROOT / 'build/browser.so', installed / BROWSER_MODULE)
            local_documents = [base / 'old.html', base / 'new.html']
            for document in local_documents:
                document.write_text('<!doctype html><title>Fixture</title><input><!-- local-generation -->')
            log = CHECKS / "browser-generations.log"
            with log.open('wb') as output:
                command = [str(executable), str(ROOT / 'build/browser.so'), str(updated),
                           str(ROOT / 'build/incompatible_browser.so'), url + '/page',
                           *(document.as_uri() for document in local_documents),
                           str(ROOT / "build/unresolved_browser.so")]
                if os.environ.get('WORMINAL_PREVIEW_DEBUG'):
                    command = ['gdb', '-batch', '--return-child-result', '-ex', 'set debuginfod enabled off',
                               '-ex', 'run', '-ex', 'thread apply all bt', '--args', *command]
                result = subprocess.run(['dbus-run-session', '--', *command],
                                        env={**env, 'GDK_BACKEND': 'x11', 'GTK_USE_PORTAL': '0'},
                                        stdout=output, stderr=output, timeout=45)
            self.assertEqual(result.returncode, 0, log.read_text(errors='replace')[-5000:])
            self.assertIn("Browser generations:", log.read_text(errors="replace"))
