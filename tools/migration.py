"""Read-only C baseline capture for the Rust migration; never publishes or touches live processes."""

import hashlib
import json
import shutil
import subprocess

from tools.build import ROOT, installation_files


def preserve_baseline():
    destination = ROOT / ".checks/c-baseline"
    # Exclusive creation prevents a later Rust build from silently replacing the C recovery artifacts.
    destination.mkdir(parents=True, exist_ok=False)
    records = []
    try:
        for source, installed, _mode in installation_files("/usr/local"):
            for kind, artifact in (("build", source), ("installed", installed)):
                if not artifact.is_file():
                    records.append({"kind": kind, "path": str(artifact), "missing": True})
                    continue
                relative = artifact.relative_to(ROOT) if kind == "build" else artifact.relative_to("/")
                target = destination / kind / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(artifact, target)
                digest = hashlib.sha256(target.read_bytes()).hexdigest()
                records.append({"kind": kind, "path": str(artifact),
                                "copy": str(target.relative_to(destination)), "sha256": digest})
        revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
        (destination / "manifest.json").write_text(json.dumps(
            {"revision": revision, "artifacts": records}, indent=2) + "\n")
    except BaseException:
        # Incomplete copies must not masquerade as a usable recovery baseline.
        shutil.rmtree(destination)
        raise
    return destination
