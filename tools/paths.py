"""Local builds and installations use the same bin/lib directories."""
import os
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BUILD = ROOT / 'build'
CHECKS = BUILD / 'checks'
if directory := os.environ.get('WORMINAL_TEST_CHECKS_DIR'):
    private = Path(directory).resolve()
    if not private.is_relative_to(CHECKS.resolve()) or private == CHECKS.resolve():
        raise ValueError('Worker diagnostics must remain below project build/checks.')
    CHECKS = private


def runtime_binary(name, *, prefix=None):
    path = Path(name)
    if path.is_absolute():
        return path
    if len(path.parts) > 1:
        return ROOT / path
    return (BUILD if prefix is None else Path(prefix)) / 'bin' / path


def module_path(name, *, prefix=None):
    return (BUILD if prefix is None else Path(prefix)) / 'lib/worminal' / name
