"""Performance gates reject regressions and invalid/mismatched measurement evidence."""
import copy
from contextlib import ExitStack
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from typer.testing import CliRunner

from tools import perf_check
from runtime import CHECKS


class PerfCheckTests(unittest.TestCase):
    def setUp(self):
        self.policy = {"workload_sha256": "fixture", "unit": "ns", "trials": 3,
                       "metrics": {"cost": {"baseline": 8, "maximum": 10}}, "informational_metrics": []}
        self.report = {"workload_sha256": "fixture", "unit": "ns", "trials": 3,
                       "medians": {"current": {"cost": 10}}, "samples": {"current": {"cost": [8, 10, 12]}}}

    def test_ceiling_is_inclusive_and_regression_fails(self):
        self.assertTrue(perf_check.evaluate(self.report, self.policy)["passed"])
        self.report["medians"]["current"]["cost"] = 11
        self.assertFalse(perf_check.evaluate(self.report, self.policy)["passed"])

    def test_historical_baseline_can_exceed_a_new_optimization_target(self):
        self.policy["metrics"]["cost"]["baseline"] = 20
        self.assertTrue(perf_check.evaluate(self.report, self.policy)["passed"])

    def test_invalid_or_missing_measurements_fail_closed(self):
        for value in (None, True, -1, float("nan"), float("inf"), "1"):
            with self.subTest(value=value):
                report = copy.deepcopy(self.report)
                report["medians"]["current"]["cost"] = value
                self.assertFalse(perf_check.evaluate(report, self.policy)["passed"])
        self.report["samples"]["current"]["cost"] = [1, float("nan"), 1]
        self.assertFalse(perf_check.evaluate(self.report, self.policy)["passed"])
        del self.report["medians"]["current"]["cost"]
        self.assertFalse(perf_check.evaluate(self.report, self.policy)["passed"])

    def test_workload_units_trial_count_and_new_metrics_need_review(self):
        for field, value in (("workload_sha256", "changed"), ("unit", "ms"), ("trials", 1)):
            report = self.report | {field: value}
            self.assertFalse(perf_check.evaluate(report, self.policy)["passed"])
        self.report["medians"]["current"]["frames"] = 1
        self.assertFalse(perf_check.evaluate(self.report, self.policy)["passed"])
        self.policy["informational_metrics"] = ["frames"]
        self.assertTrue(perf_check.evaluate(self.report, self.policy)["passed"])

    def test_failed_benchmark_retains_results_and_other_suite_still_runs(self):
        CHECKS.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="perf-gate-", dir=CHECKS) as directory:
            directory = Path(directory)
            limits = directory / "limits.json"
            limits.write_text(json.dumps({"schema_version": 1,
                              "suites": {name: self.policy for name in ("daemon", "network")}}))
            with patch.object(perf_check, "CHECKS", directory), patch.object(
                    perf_check.daemon_perf, "measure", side_effect=[
                        subprocess.CalledProcessError(1, ["benchmark"]), self.report]) as measure:
                self.assertEqual(perf_check.check(limits), 1)
            self.assertEqual(measure.call_count, 2)
            saved = json.loads((directory / "perf-check.json").read_text())
            self.assertFalse(saved["passed"])
            self.assertTrue(saved["suites"]["network"]["passed"])
            self.assertEqual(json.loads(limits.read_text()), saved["limits"])

    def test_checked_in_policy_has_nonempty_reviewed_limits(self):
        policy = json.loads(perf_check.LIMITS.read_text())
        self.assertEqual(set(policy["suites"]), {"daemon", "network"})
        for suite in policy["suites"].values():
            self.assertTrue(suite["metrics"])
            for budget in suite["metrics"].values():
                self.assertTrue(perf_check.number(budget["baseline"]))
                self.assertTrue(perf_check.number(budget["maximum"]))

    def test_cli_exits_nonzero_for_over_budget_measurements_and_keeps_evidence(self):
        import manage
        CHECKS.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="perf-cli-", dir=CHECKS) as directory:
            directory = Path(directory)
            limits = directory / "limits.json"
            policy = {"schema_version": 1, "suites": {name: self.policy for name in ("daemon", "network")}}
            limits.write_text(json.dumps(policy))
            self.report["medians"]["current"]["cost"] = 11
            with patch.object(perf_check, "CHECKS", directory), patch.object(
                    perf_check.daemon_perf, "measure", return_value=self.report):
                result = CliRunner().invoke(manage.app, ["check-perf", "--limits", str(limits)])
            self.assertEqual(result.exit_code, 1, result.output)
            saved = json.loads((directory / "perf-check.json").read_text())
            self.assertFalse(saved["passed"])
            self.assertEqual(saved["suites"]["daemon"]["metrics"]["cost"]["observed"], 11)
            self.assertEqual(json.loads(limits.read_text()), policy)

    def test_native_phase_propagates_performance_failure(self):
        import manage
        with ExitStack() as stack:
            stack.enter_context(patch("tools.engine_checks.check", return_value=0))
            for name in ("run_rust_lint", "run_rust_workspace_checks", "run_rust_process_checks",
                         "run_rust_frontend_tests"):
                stack.enter_context(patch.object(manage, name, return_value=0))
            check = stack.enter_context(patch.object(manage, "run_perf_check", return_value=1))
            self.assertEqual(manage.run_native_checks(), 1)
            check.assert_called_once_with()

    def test_python_cli_enforces_gate_even_for_filtered_tests(self):
        import manage
        for status in (0, 1):
            with self.subTest(status=status), patch.object(manage, "run_perf_check", return_value=status) as gate:
                with patch("tools.test_runner.run_python", return_value=0) as python:
                    result = CliRunner().invoke(manage.app, ["check-python", "--module", "test_perf_check",
                                                            "--match", "ceiling", "--jobs", "1"])
                self.assertEqual(result.exit_code, status, result.output)
                gate.assert_called_once_with()
                if status:
                    python.assert_not_called()
                else:
                    python.assert_called_once_with(jobs=1, match="ceiling", modules=["test_perf_check"])
