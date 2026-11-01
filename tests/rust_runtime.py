"""Copy a complete native Rust application into a project-owned fixture directory."""
from pathlib import Path
import shutil
import sys
import subprocess
import time

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
from tools.build import RUST_BROWSER_MODULE, RUST_FRONTEND_MODULE


def copy_runtime(directory, *, browser=False):
    base = Path(directory)
    (base / 'build').mkdir(exist_ok=True)
    for name in ('worminal-rust', 'worminald'):
        shutil.copy2(ROOT / name, base / name)
    modules = [RUST_FRONTEND_MODULE, *([RUST_BROWSER_MODULE] if browser else [])]
    for name in modules: shutil.copy2(ROOT / 'build' / name, base / 'build' / name)
    return base / 'worminal-rust'

def window(env, process):
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise AssertionError(f"Rust app exited: {process.stderr.read().decode(errors='replace')}")
        result = subprocess.run(['xdotool', 'search', '--onlyvisible', '--pid', str(process.pid)],
                                env=env, capture_output=True, text=True, timeout=2)
        if result.returncode == 0:
            return result.stdout.splitlines()[0]
        time.sleep(.02)
    raise AssertionError('Rust window did not open')



def stop_application(process):
    if process.poll() is None: process.terminate()
    _, error = process.communicate(timeout=5)
    if error:
        with (ROOT / '.checks/rust-app-ui.log').open('ab') as log: log.write(error)
