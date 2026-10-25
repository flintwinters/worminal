"""Privacy detection and remote-proof configuration regressions."""

import contextlib
import importlib.util
import io
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from tools.privacy import check_privacy, findings
from tools.git_index import tracked_files


class PrivacyTests(unittest.TestCase):
    def test_cli_scans_requested_repository_instead_of_parent(self):
        root = Path(__file__).resolve().parent.parent
        checks = root / ".checks"
        checks.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=checks) as directory:
            repository = Path(directory)
            subprocess.run(["git", "init", "-q", directory], check=True)
            (repository / "fixture.txt").write_text("ghp_" + "a" * 30)
            subprocess.run(["git", "-C", directory, "add", "fixture.txt"], check=True)
            result = subprocess.run([sys.executable, str(root / "manage.py"), "check-privacy",
                                     "--repository", directory], capture_output=True, text=True)
            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertIn("fixture.txt:1: service credential", result.stdout)
            self.assertNotIn("ghp_" + "a" * 30, result.stdout)

    def test_forced_private_file_is_rejected_even_without_recognized_credentials(self):
        root = Path(__file__).resolve().parent.parent
        with patch("tools.privacy.git", side_effect=[b"100644 abc 0\t.env\0", b"CUSTOM_SETTING=opaque"]):
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertTrue(check_privacy(root, staged=True))

    def test_history_rejects_non_anonymous_names_without_email_addresses(self):
        calls = [b"", b"", b"abc\n", b"", b"Person\nanonymous\nPerson\nanonymous\n"]
        with patch("tools.privacy.git", side_effect=calls):
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertTrue(check_privacy(Path.cwd(), history=True))

    def test_staged_scan_uses_index_even_when_worktree_is_clean(self):
        root = Path(__file__).resolve().parent.parent
        secret = b"ghp_" + b"a" * 30
        with patch("tools.privacy.git", side_effect=[b"100644 abc 0\tREADME.md\0", secret]) as git:
            with contextlib.redirect_stdout(io.StringIO()) as output:
                self.assertTrue(check_privacy(root, staged=True))
            self.assertEqual(git.call_args_list[-1].args, ("show", ":README.md"))
            self.assertNotIn(secret.decode(), output.getvalue())

    def test_gitlinks_are_not_read_as_parent_repository_files(self):
        index = b"160000 abc 0\tfile-explorer\0" + b"100644 def 0\tREADME.md\0"
        self.assertEqual(list(tracked_files(index)), ["README.md"])
        with patch("tools.privacy.git", side_effect=[index, b"public text"]) as git:
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertFalse(check_privacy(Path.cwd(), staged=True))
            self.assertEqual(git.call_args_list[-1].args, ("show", ":README.md"))

    def test_index_paths_preserve_tabs_and_newlines(self):
        self.assertEqual(list(tracked_files(b"100644 abc 0\ta\tb\nc\0")), ["a\tb\nc"])

    def test_personal_identity_override_is_rejected(self):
        identity = b"Person <person@" + b"mail.invalid> 1 +0000"
        with patch("tools.privacy.git", side_effect=[b"", identity, identity]):
            with contextlib.redirect_stdout(io.StringIO()) as output:
                self.assertTrue(check_privacy(Path.cwd(), identities=True))
            self.assertNotIn(identity.decode(), output.getvalue())

    def test_commit_message_is_scanned(self):
        message = unittest.mock.Mock()
        message.read_bytes.return_value = b"contact person@" + b"mail.invalid"
        with patch("tools.privacy.git", return_value=b""):
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertTrue(check_privacy(Path.cwd(), message_file=message))

    def test_sensitive_categories_without_exposing_values(self):
        samples = {
            "private key": b"-----BEGIN " + b"OPENSSH PRIVATE KEY-----",
            "service credential": b"ghp_" + b"a" * 30,
            "credential assignment": b'client_secret=' + b'"' + b"a" * 30 + b'"',
            "personal home path": b"/home/" + b"person/project/",
            "private network address": b"192." + b"168.12.34",
            "email address": b"person@" + b"mail.invalid",
            "URL credential": b"https://person:" + b"credential@" + b"example.org",
        }
        for category, sample in samples.items():
            with self.subTest(category=category):
                self.assertIn((category, 2), findings(b"first line\n" + sample))
                self.assertNotIn(sample.decode(), repr(findings(sample)))

    def test_public_examples_and_configuration_are_not_private(self):
        self.assertEqual(findings(b"user@host https://example.org /opt/projects/worminal "
                                  b"WORMINAL_CINNAMON_HOST PASSWORD=getenv(name)"), [])

    def test_unconfigured_proof_never_opens_display_or_ssh(self):
        path = Path(__file__).with_name("proof_cinnamon.py")
        spec = importlib.util.spec_from_file_location("cinnamon_privacy_test", path)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        for host in ("", "-unexpected-option"):
            with self.subTest(host=host), patch.dict(module.os.environ, {"WORMINAL_CINNAMON_HOST": host}):
                with patch.object(module, "isolated_display") as display:
                    with contextlib.redirect_stderr(io.StringIO()):
                        self.assertEqual(module.main(), 1)
                    display.assert_not_called()
