"""Disposable historical test fixtures reconstructed from Git, never installable recovery releases."""
import atexit
import importlib.util
import io
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parent.parent
REVISION = 'c1d4f8e'
_module = None
_directory = None
_links = {}


def cleanup():
    global _module, _directory
    for path, expected in reversed(list(_links.items())):
        if path.is_symlink() and path.readlink() == expected:
            path.unlink()
    _links.clear()
    _module = None
    if _directory is not None:
        _directory.cleanup()
        _directory = None


def extract(revision, destination):
    archive = subprocess.check_output(['git', 'archive', revision, 'src', 'tests', 'tools/build.py', 'assets'],
                                      cwd=ROOT)
    with tarfile.open(fileobj=io.BytesIO(archive)) as source:
        for entry in source:
            name = Path(entry.name)
            if name.is_absolute() or '..' in name.parts:
                raise ValueError('Unsafe historical fixture path')
            if entry.isfile():
                target = destination / name
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(source.extractfile(entry).read())


def module():
    global _module, _directory
    if _module is not None:
        return _module
    (ROOT / 'build').mkdir(exist_ok=True)
    _directory = tempfile.TemporaryDirectory(prefix='.historical-', dir=ROOT / 'build')
    root = Path(_directory.name)
    extract(REVISION, root)
    # Existing regression harnesses include private historical headers and adapters. Expose them only
    # during this check process; current engine sources always take precedence over historical files.
    for source in (root / 'src').rglob('*'):
        if not source.is_file():
            continue
        target = ROOT / source.relative_to(root)
        if not target.exists():
            target.parent.mkdir(parents=True, exist_ok=True)
            if target.is_symlink():
                target.unlink()
            target.symlink_to(source)
            _links[target] = source
    specification = importlib.util.spec_from_file_location('worminal_historical_build', root / 'tools/build.py')
    _module = importlib.util.module_from_spec(specification)
    specification.loader.exec_module(_module)
    return _module


def target(name):
    if name in ('worminal', 'worminald', 'worminal-rust', 'worminal-preview', 'worminald-preview'):
        raise ValueError('Historical fixtures cannot replace current application binaries')
    historical = module()
    from tools import build
    build.generate_theme()
    (historical.ROOT / 'build').mkdir(exist_ok=True)
    shutil.copy2(ROOT / 'build/theme.h', historical.ROOT / 'build/theme.h')
    builder = historical.Builder()
    output = name
    if name == 'build/worminal-gtk-c':
        builder.target('build/frontend.so')
        builder.target('build/browser.so')
        output = 'worminal-gtk-c'
        packages = ('glib-2.0', 'gmodule-2.0')
        builder.cflags = [*historical.CPPFLAGS, *historical.pkg_flags('--cflags', packages)]
        builder.program(output, ['src/gtk/launcher.c', 'src/gtk/windows_forward.c',
                                'src/workspace/workspace_socket.c', 'src/workspace/wire.c'],
                        flags=['-DWORMINAL_PREVIEW_PROTOCOL'], libs=historical.pkg_flags('--libs', packages))
    else:
        if name == 'build/render_probe':
            builder.target('build/frontend.so')
        builder.target(name)
    alias = ROOT / name
    alias.parent.mkdir(parents=True, exist_ok=True)
    alias.unlink(missing_ok=True)
    alias.symlink_to(historical.ROOT / output)
    _links[alias] = historical.ROOT / output
    # The painter dynamically resolves the reference frontend next to its probe.
    if name == 'build/render_probe':
        frontend = ROOT / 'build/frontend.so'
        frontend.unlink(missing_ok=True)
        frontend.symlink_to(historical.ROOT / 'build/frontend.so')
        _links[frontend] = historical.ROOT / "build/frontend.so"


def remove_fixture_links(root=None):
    """Recover interrupted checks without deleting a user's replacement source or executable."""
    root = ROOT if root is None else root
    candidates = [*root.glob('worminal*-x11'), *(root / 'src').rglob('*')]
    for path in candidates:
        if not path.is_symlink():
            continue
        target = path.readlink()
        if (target.is_relative_to(root / 'build')
                and target.relative_to(root / 'build').parts[0].startswith('.historical-')):
            path.unlink()


atexit.register(cleanup)
