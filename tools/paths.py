"""One layout for disposable builds and native runtime prefixes, matching /usr/local installation."""
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BUILD = ROOT / 'build'
CHECKS = BUILD / 'checks'


def runtime_binary(name, *, prefix=None):
    path = Path(name)
    if path.is_absolute():
        return path
    if len(path.parts) > 1:
        return ROOT / path  # Explicit repository-relative executable overrides remain supported.
    return (BUILD if prefix is None else Path(prefix)) / 'bin' / path


def module_path(name, *, prefix=None):
    return (BUILD if prefix is None else Path(prefix)) / 'lib/worminal' / name
