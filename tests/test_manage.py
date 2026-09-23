import os
from pathlib import Path
import unittest
from unittest import mock

import manage


class RustToolchainTest(unittest.TestCase):
    def test_rustup_precedes_system_cargo_in_existing_shell(self):
        with mock.patch.dict(os.environ, {"PATH": "/usr/bin"}):
            with mock.patch.object(Path, "home", return_value=Path("/home/example")):
                with mock.patch.object(Path, "is_file", return_value=True):
                    path = manage.rust_env()["PATH"].split(os.pathsep)
        self.assertEqual(path[:2], ["/home/example/.cargo/bin", "/usr/bin"])
