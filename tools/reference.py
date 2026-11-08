"""Build historical source fixtures from Git for tests, then remove temporary files."""
import atexit
import importlib.util
import io
from pathlib import Path
from tools.paths import CHECKS, runtime_binary
import shutil
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parent.parent
REVISION = 'c1d4f8e'
# Tests built only against the historical code whose maintained source is the working tree. Their
# checks and tolerances evolve with the project; the historical revision supplies only the product code.
MAINTAINED_TESTS = ('tests/preview.c', 'tests/preview_text.c')
_module = None
_directory = None
_links = {}
_parents = set()


def cleanup():
    global _module, _directory
    for path, expected in reversed(list(_links.items())):
        if path.is_symlink() and path.readlink() == expected:
            path.unlink()
    _links.clear()
    for path in sorted(_parents, key=lambda path: len(path.parts), reverse=True):
        try:
            path.rmdir()
        except OSError:
            pass  # Leave the directory if files have been added to it.
    _parents.clear()
    _module = None
    if _directory is not None:
        _directory.cleanup()
        _directory = None


def extract(revision, destination, paths=('src', 'tests', 'tools/build.py', 'assets')):
    archive = subprocess.check_output(['git', 'archive', revision, *paths],
                                      cwd=ROOT)
    with tarfile.open(fileobj=io.BytesIO(archive)) as source:
        for entry in source:
            name = Path(entry.name)
            if name.is_absolute() or '..' in name.parts:
                raise ValueError('Unsafe historical fixture path')
            if entry.isfile():
                target = destination / name
                target.parent.mkdir(parents=True, exist_ok=True)
                data = source.extractfile(entry).read()
                if target.suffix in ('.c', '.h', '.py'):
                    data = data.replace(b'.checks', CHECKS.relative_to(ROOT).as_posix().encode())
                if name.as_posix() in MAINTAINED_TESTS:
                    data = (ROOT / name).read_bytes()
                target.write_bytes(data)


def module():
    global _module, _directory
    if _module is not None:
        return _module
    (ROOT / 'build').mkdir(exist_ok=True)
    _directory = tempfile.TemporaryDirectory(prefix='.historical-', dir=ROOT / 'build')
    root = Path(_directory.name)
    extract(REVISION, root)
    # Some tests still include the old C headers and source files. Link in missing files for the test
    # run, then remove the links in cleanup(). Never replace files already present in src/.
    for source in (root / 'src').rglob('*'):
        if not source.is_file():
            continue
        target = ROOT / source.relative_to(root)
        if not target.exists():
            parent = target.parent
            while not parent.exists():
                _parents.add(parent)
                parent = parent.parent
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
    alias = runtime_binary(name) if "/" not in name else ROOT / name
    alias.parent.mkdir(parents=True, exist_ok=True)
    alias.unlink(missing_ok=True)
    alias.symlink_to(historical.ROOT / output)
    _links[alias] = historical.ROOT / output
    # The rendering test loads the old frontend from build/frontend.so.
    if name == 'build/render_probe':
        frontend = ROOT / 'build/frontend.so'
        frontend.unlink(missing_ok=True)
        frontend.symlink_to(historical.ROOT / 'build/frontend.so')
        _links[frontend] = historical.ROOT / "build/frontend.so"


def remove_fixture_links(root=None):
    """Remove links left by interrupted tests without deleting other files."""
    root = ROOT if root is None else root
    candidates = [*root.glob('worminal*-x11'), *(root / 'build/bin').glob('*'), *(root / 'src').rglob('*')]
    for path in candidates:
        if not path.is_symlink():
            continue
        target = path.readlink()
        if (target.is_relative_to(root / 'build')
                and target.relative_to(root / 'build').parts[0].startswith('.historical-')):
            path.unlink()
            parent = path.parent
            while parent.is_relative_to(root / 'src') and parent != root / 'src':
                try:
                    parent.rmdir()
                except OSError:
                    break
                parent = parent.parent


atexit.register(cleanup)
