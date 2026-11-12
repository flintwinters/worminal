"""Daily auto-update: installs a changed tip once, never installs a failing one, schedules 03:00."""

from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from tools import auto_update
from tools.paths import CHECKS


def git(*args, cwd):
    subprocess.run(["git", "-c", "user.name=t", "-c", "user.email=test", *args], cwd=cwd, check=True,
                   capture_output=True)


class UpdateTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(dir=CHECKS, prefix="auto-update-")
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        self.origin, self.clone, self.stamp = root / "origin", root / "work/clone", root / "work/stamp"
        self.origin.mkdir()
        git("init", "-q", "-b", "main", cwd=self.origin)
        self.commit("one")
        self.steps = []

    def commit(self, text):
        (self.origin / "file").write_text(text)
        git("add", "file", cwd=self.origin)
        git("commit", "-q", "-m", text, cwd=self.origin)

    def update(self, failing=None):
        def manage(*args, cwd, sudo=False):
            self.steps.append((args[0], sudo))
            return 1 if args[0] == failing else 0
        with patch.object(auto_update, "manage", manage):
            return auto_update.update(str(self.origin), "main", self.clone, self.stamp, "/prefix")

    def test_installs_each_new_tip_once_in_order_with_sudo_only_for_install(self):
        self.assertEqual(self.update(), 0)
        self.assertEqual(self.steps, [("build", False), ("setup-preview-audio", False),
                                     ("check", False), ("install", True)])
        self.update()
        self.assertEqual(len(self.steps), 4)
        self.commit("two")
        self.update()
        self.assertEqual(len(self.steps), 8)
        self.assertEqual((self.clone / "file").read_text(), "two")

    def test_failing_check_never_installs_and_is_retried_next_night(self):
        self.assertEqual(self.update(failing="check"), 1)
        self.assertEqual(self.steps, [("build", False), ("setup-preview-audio", False), ("check", False)])
        self.assertFalse(self.stamp.exists())
        self.update()
        self.assertIn(("install", True), self.steps)

    def test_failed_installed_verification_does_not_mark_tip_successful(self):
        def manage(*args, **kwargs):
            self.steps.append(args[0])
            return 1 if args[0] == "check-install" else 0
        with patch.object(auto_update, "manage", manage):
            result = auto_update.update(str(self.origin), "main", self.clone, self.stamp)
        self.assertEqual(result, 1)
        self.assertEqual(self.steps, ["build", "setup-preview-audio", "check", "install", "check-install"])
        self.assertFalse(self.stamp.exists())


class ScheduleTests(unittest.TestCase):
    def test_timer_fires_at_three_and_catches_up_missed_runs(self):
        timer = auto_update.units()[f"{auto_update.UNIT}.timer"]
        self.assertIn("OnCalendar=*-*-* 03:00:00", timer)
        self.assertIn("Persistent=true", timer)
        self.assertIn("manage.py auto-update", auto_update.units()[f"{auto_update.UNIT}.service"])


class PublishTests(unittest.TestCase):
    def test_verifies_installed_files_after_publication(self):
        with patch.object(auto_update, "manage", return_value=0) as manage:
            self.assertEqual(auto_update.publish(), 0)
        self.assertEqual([call.args[0] for call in manage.call_args_list],
                         ["build", "setup-preview-audio", "check", "install", "check-install"])
        self.assertEqual([call.kwargs["sudo"] for call in manage.call_args_list],
                         [False, False, False, True, False])

    def test_failure_stops_all_later_steps(self):
        for failed in range(5):
            with self.subTest(failed=failed), patch.object(
                    auto_update, "manage", side_effect=[0] * failed + [7]) as manage:
                self.assertEqual(auto_update.publish(), 7)
                self.assertEqual(manage.call_count, failed + 1)


if __name__ == "__main__":
    unittest.main()
