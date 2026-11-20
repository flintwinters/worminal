"""Runtime configuration for tools outside Rust.

`worminald config --json` is the only resolver (worminal.yaml layered over Alacritty config); this module
asks it for the answer.
"""

import json
import os
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent


def settings(defaults=False, environment=None):
    """Resolved configuration; `defaults` ignores every file and reports the built-in values.

    `environment` overlays variables for the resolver, e.g. to select one Alacritty file."""
    from tools.build import Builder
    daemon = Builder().rust_artifact("worminal-daemon")
    arguments = [str(daemon), "config", "--json", *(["--defaults"] if defaults else [])]
    result = subprocess.run(arguments, capture_output=True, text=True, check=False,
                            env={**os.environ, **(environment or {})})
    if result.returncode:
        raise ValueError(result.stderr.strip() or "worminald config failed")
    return json.loads(result.stdout)
