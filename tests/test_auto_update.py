"""Daily auto-update: installs a changed tip once, never installs a failing one, schedules 03:00."""

from pathlib import Path
from contextlib import contextmanager
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
        self.installed = root / "work/installed-commit"
        self.local = root / "checkout"
        (root / "work").mkdir()
        self.origin.mkdir()
        git("init", "-q", "-b", "main", cwd=self.origin)
        self.commit("one")
        self.steps = []

    def commit(self, text):
        (self.origin / "file").write_text(text)
        git("add", "file", cwd=self.origin)
        git("commit", "-q", "-m", text, cwd=self.origin)

    def update(self, failing=None):
        def delegate(cwd, prefix):
            self.steps.append((Path(cwd), prefix))
            return 1 if failing else 0
        with patch.object(auto_update, "delegate", delegate), patch.object(
                auto_update, "notify_failure") as notify:
            self.notified = notify
            return auto_update.update(str(self.origin), "main", self.clone, self.stamp, "/prefix", self.installed, self.local)

    def test_installs_each_new_tip_once_by_running_the_clones_own_update(self):
        self.assertEqual(self.update(), 0)
        self.assertEqual(self.steps, [(self.clone, "/prefix")])
        self.update()
        self.assertEqual(len(self.steps), 1)
        self.commit("two")
        self.update()
        self.assertEqual(len(self.steps), 2)
        self.assertEqual((self.clone / "file").read_text(), "two")

    def test_failing_update_never_advances_stamp_notifies_and_is_retried_next_night(self):
        self.assertEqual(self.update(failing=True), 1)
        self.assertFalse(self.stamp.exists())
        self.notified.assert_called_once()
        self.assertEqual(self.update(), 0)
        self.assertTrue(self.stamp.exists())

    def checkout(self, text=None):
        """A developer checkout whose HEAD is recorded as the installed build, optionally with unpushed work."""
        git("clone", "-q", str(self.origin), str(self.local), cwd=self.origin.parent)
        if text:
            (self.local / "file").write_text(text)
            git("commit", "-q", "-am", text, cwd=self.local)
        self.installed.write_text(subprocess.run(["git", "rev-parse", "HEAD"], cwd=self.local, text=True,
                                                 capture_output=True).stdout)

    def test_tip_missing_the_installed_build_is_skipped_not_rolled_back(self):
        # The installed build came from work that never reached the branch; installing the branch would undo it.
        self.checkout("unpushed work")
        self.commit("two")
        self.assertEqual(self.update(), 0)
        self.assertEqual(self.steps, [])
        self.assertFalse(self.stamp.exists())

    def test_tip_containing_the_installed_build_is_installed(self):
        self.checkout()
        self.commit("two")
        self.assertEqual(self.update(), 0)
        self.assertEqual(len(self.steps), 1)


class ScheduleTests(unittest.TestCase):
    def test_timer_fires_at_three_and_catches_up_missed_runs(self):
        timer = auto_update.units()[f"{auto_update.UNIT}.timer"]
        self.assertIn("OnCalendar=*-*-* 03:00:00", timer)
        self.assertIn("Persistent=true", timer)
        self.assertIn("manage.py auto-update", auto_update.units()[f"{auto_update.UNIT}.service"])


class PublishTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(dir=CHECKS, prefix="publish-")
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        # A real update holds the lock of the checkout it runs in; these tests must never share it.
        for target in ("tools.running.report", "tools.update_storage.release_stale"):
            patcher = patch(target)
            patcher.start()
            self.addCleanup(patcher.stop)
        patcher = patch.object(auto_update, "git", return_value="abc123")
        patcher.start()
        self.addCleanup(patcher.stop)

    def publish(self):
        return auto_update.publish(self.root, installed=self.root / "installed")

    def test_verifies_installed_files_after_publication_and_records_the_commit(self):
        with patch.object(auto_update, "manage", return_value=0) as manage:
            self.assertEqual(self.publish(), 0)
        self.assertEqual([call.args[0] for call in manage.call_args_list],
                         ["build", "setup-check-runtime", "check", "install", "check-install"])
        self.assertEqual([call.kwargs["sudo"] for call in manage.call_args_list],
                         [False, False, False, True, False])
        self.assertEqual((self.root / "installed").read_text(), "abc123\n")

    def test_failure_stops_all_later_steps_and_records_nothing(self):
        for failed in range(5):
            with self.subTest(failed=failed), patch.object(
                    auto_update, "manage", side_effect=[0] * failed + [7]) as manage:
                self.assertEqual(self.publish(), 7)
                self.assertEqual(manage.call_count, failed + 1)
                self.assertFalse((self.root / "installed").exists())


class ManageTests(unittest.TestCase):
    def test_standalone_owners_retire_before_verification_storage_is_released(self):
        events = []
        directory = Path('/project/build/checks/private')

        @contextmanager
        def environment(cwd):
            yield {"WORMINAL_TEST_CHECKS_DIR": str(directory)}
            events.append("released")

        with patch("tools.update_storage.verification_environment", environment), patch(
                "tools.auto_update.subprocess.Popen") as spawn, patch(
                "tools.test_runner.retire_worker", side_effect=lambda *args: events.append("retired")) as retire:
            spawn.return_value.wait.return_value = 7
            self.assertEqual(auto_update.manage("check", cwd=Path('/project')), 7)
            retire.assert_called_once_with(spawn.return_value, directory)
        self.assertEqual(events, ["retired", "released"])


if __name__ == "__main__":
    unittest.main()
