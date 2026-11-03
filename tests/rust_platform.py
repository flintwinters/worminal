"""Exercise locked Rust UI bindings without using the desktop or real browser profiles."""
from runtime import CHECKS


from pathlib import Path
import subprocess
import tempfile

from x11 import isolated_display

ROOT = Path(__file__).resolve().parent.parent


def main():
    checks = CHECKS
    checks.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="rust-platform-", dir=checks) as directory:
        with isolated_display() as env:
            for name in ("XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_CONFIG_HOME", "XDG_STATE_HOME"):
                env[name] = str(Path(directory) / name.lower())
            env.pop("WAYLAND_DISPLAY", None)
            env["GDK_BACKEND"] = "x11"
            subprocess.run([str(ROOT / "build/cargo/debug/worminal-platform")],
                           cwd=ROOT, env=env, check=True, timeout=30)


if __name__ == "__main__":
    main()
