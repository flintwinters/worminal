"""file-explorer build freshness: one definition shared by update preparation and the preview check."""
import subprocess
import sys

from tools.paths import ROOT

EXPLORER = ROOT / "file-explorer"
DIST = EXPLORER / "dist/index.html"


def _newest_source():
    return max((path.stat().st_mtime for tree in ("apps", "packages") for path in (EXPLORER / tree).rglob("*")
                if path.is_file() and "node_modules" not in path.parts), default=0)


def present():
    return (EXPLORER / "manage.py").is_file()


def dependencies_stale():
    installed = EXPLORER / "node_modules/.package-lock.json"
    return not installed.is_file() or installed.stat().st_mtime < (EXPLORER / "package-lock.json").stat().st_mtime


def dist_stale():
    """True when the artifact is missing or older than any source; an old one fails checks with unrelated timeouts."""
    return not DIST.is_file() or DIST.stat().st_mtime < _newest_source()


def prepare():
    """Install pinned dependencies and rebuild only when needed; the explorer is optional (submodule)."""
    if not present():
        return
    if dependencies_stale():
        subprocess.run(["npm", "ci"], cwd=EXPLORER, check=True)
    if dist_stale():
        subprocess.run([sys.executable, str(ROOT / "manage.py"), "explorer-build"], check=True)
