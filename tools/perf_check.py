"""Fixed performance ceilings for existing release workloads; never ratchet limits automatically."""
import json
import math
import subprocess
from pathlib import Path

from tools import daemon_perf, timing
from tools.paths import ROOT, CHECKS

LIMITS = ROOT / "tools/perf_limits.json"


def number(value):
    return type(value) in (int, float) and math.isfinite(value) and value >= 0


def evaluate(report, policy):
    """Fail closed on changed workloads, missing metrics and invalid numeric observations."""
    errors, results = [], {}
    if report.get("workload_sha256") != policy["workload_sha256"]:
        errors.append("workload changed; review and remeasure the baseline")
    if report.get("unit") != policy["unit"]:
        errors.append("measurement units changed")
    if report.get("trials", 0) < policy["trials"]:
        errors.append("insufficient measured trials")
    values = report.get("medians", {}).get("current", {})
    samples = report.get("samples", {}).get("current", {})
    unexpected = values.keys() - policy["metrics"].keys() - set(policy["informational_metrics"])
    if unexpected:
        errors.append(f"metrics need a budget decision: {sorted(unexpected)}")
    for name, budget in policy["metrics"].items():
        maximum = budget["maximum"]
        if not number(maximum) or not number(budget["baseline"]) or budget["baseline"] > maximum:
            raise ValueError(f"Invalid performance budget: {name}")
        value, raw = values.get(name), samples.get(name, [])
        valid = (number(value) and len(raw) == report.get("trials") and all(number(item) for item in raw))
        passed = valid and value <= maximum
        results[name] = {"observed": value, "maximum": maximum, "passed": passed}
        if not passed:
            errors.append(f"{name}: {value!r} exceeds {maximum} or has missing/invalid samples")
    return {"passed": not errors, "metrics": results, "errors": errors}


def check(limits=LIMITS):
    policy = json.loads(Path(limits).read_text())
    if policy.get("schema_version") != 1 or set(policy["suites"]) != {"daemon", "network"}:
        raise ValueError("Expected performance limit schema 1 with daemon and network suites")
    results = {}
    destination = CHECKS / "perf-check.json"
    for name in ("daemon", "network"):
        budget = policy["suites"][name]
        if type(budget["trials"]) is not int or budget["trials"] < 3 or not budget["metrics"]:
            raise ValueError(f"Invalid trial count or empty performance budget: {name}")
        try:
            with timing.span(f"performance ceiling: {name}", "benchmark"):
                report = daemon_perf.measure("check", budget["trials"], network=name == "network")
            results[name] = evaluate(report, budget)
        except (RuntimeError, OSError, ValueError, subprocess.SubprocessError) as error:
            results[name] = {"passed": False, "errors": [str(error)]}
        # Preserve partial results if a later benchmark is interrupted or fails.
        destination.write_text(json.dumps({"limits": policy, "suites": results,
                                          "passed": len(results) == 2 and
                                          all(item["passed"] for item in results.values())}, indent=2) + "\n")
        print(json.dumps({"suite": name, **results[name]}), flush=True)
    print(f"Saved {destination.relative_to(ROOT)}")
    return int(not all(item["passed"] for item in results.values()))
