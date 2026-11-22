"""file-explorer build freshness: one definition shared by update preparation and the preview check."""
import json
import subprocess
import sys

from tools.paths import ROOT

EXPLORER = ROOT / "file-explorer"
DIST = EXPLORER / "dist/index.html"
# The host inputs of the last parent build, and the artifact they produced.
STAMP = ROOT / "build/explorer-inputs.json"


def environment():
    """Host inputs the explorer embeds at build time: the shared palette and the terminal's resolved font."""
    from tools.theme import explorer_font
    return {"WORMINAL_PALETTE": str(ROOT / "assets/palette.json"), "WORMINAL_FONT_JSON": json.dumps(explorer_font())}


def _stamp(inputs):
    palette = inputs["WORMINAL_PALETTE"]
    return {"font": inputs["WORMINAL_FONT_JSON"], "palette": open(palette, encoding="utf-8").read(),
            "dist_mtime_ns": DIST.stat().st_mtime_ns}


def record(inputs):
    """Remember that DIST embeds `inputs`; call after a successful parent build."""
    STAMP.parent.mkdir(parents=True, exist_ok=True)
    STAMP.write_text(json.dumps(_stamp(inputs)))


def _newest_source():
    return max((path.stat().st_mtime for tree in ("apps", "packages") for path in (EXPLORER / tree).rglob("*")
                if path.is_file() and "node_modules" not in path.parts), default=0)


def present():
    return (EXPLORER / "manage.py").is_file()


def dependencies_stale():
    installed = EXPLORER / "node_modules/.package-lock.json"
    return not installed.is_file() or installed.stat().st_mtime < (EXPLORER / "package-lock.json").stat().st_mtime


def dist_stale(inputs=None):
    """True when the artifact is missing, older than any source (an old one fails checks with unrelated timeouts), or
    not built by the parent with the current host `inputs`. The submodule's standalone build embeds no host font or
    palette, and a changed terminal font must reach the explorer, so mtimes alone cannot tell."""
    if not DIST.is_file() or DIST.stat().st_mtime < _newest_source():
        return True
    try:
        return json.loads(STAMP.read_text()) != _stamp(inputs or environment())
    except (OSError, ValueError):
        return True


def prepare():
    """Install pinned dependencies and rebuild only when needed; the explorer is optional (submodule)."""
    if not present():
        return
    if dependencies_stale():
        subprocess.run(["npm", "ci"], cwd=EXPLORER, check=True)
    if dist_stale():
        subprocess.run([sys.executable, str(ROOT / "manage.py"), "explorer-build"], check=True)
