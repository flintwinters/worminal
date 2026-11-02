"""Local builds and installations use the same bin/lib directories."""
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BUILD = ROOT / 'build'
CHECKS = BUILD / 'checks'


def runtime_binary(name, *, prefix=None):
    path = Path(name)
    if path.is_absolute():
        return path
    if len(path.parts) > 1:
        return ROOT / path
    return (BUILD if prefix is None else Path(prefix)) / 'bin' / path


def module_path(name, *, prefix=None):
    return (BUILD if prefix is None else Path(prefix)) / 'lib/worminal' / name
