"""Read-only recovery captures for the Rust migration; never publishes or changes live processes."""

import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import uuid

from tools.build import ROOT, installation_files


def prepare_cutover():
    """Capture recoverable installed artifacts and read-only live state before publication."""
    from tools.daemon_upgrade import connect, catalog, identity, terminal_descriptors
    directory = ROOT / '.checks' / ('cutover-' + uuid.uuid4().hex)
    directory.mkdir(mode=0o700, parents=True)
    report = {'artifacts': [], 'owners': [], 'windows': {}, 'workspaces': {}}
    installed = Path('/usr/local')
    paths = [installed / 'bin' / name for name in ('worminal', 'worminald', 'worminal-x11', 'worminald-x11')]
    paths += list((installed / 'lib/worminal').glob('*.so'))
    for path in paths:
        if not path.is_file(): continue
        target = directory / path.relative_to('/')
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(path, target)
        report['artifacts'].append({'path': str(path), 'copy': str(target.relative_to(directory)),
                                    'mode': path.stat().st_mode & 0o777,
                                    'sha256': hashlib.sha256(target.read_bytes()).hexdigest()})
    scopes = {os.environ.get('WORMINAL_SHARED_SOCKET_SCOPE', '')}
    for process in Path('/proc').iterdir():
        if not process.name.isdecimal(): continue
        try:
            if process.stat().st_uid != os.getuid(): continue
            executable = os.readlink(process / 'exe')
            if Path(executable.removesuffix(' (deleted)')).name not in (
                    'worminal', 'worminal-preview', 'worminal-x11'): continue
            environment = dict(entry.split(b'=', 1) for entry in (process / 'environ').read_bytes().split(b'\0')
                               if b'=' in entry)
            scope = os.fsdecode(environment.get(b'WORMINAL_SHARED_SOCKET_SCOPE', b''))
            protocol = 1 if Path(executable.removesuffix(' (deleted)')).name.endswith('-x11') else 2
            if protocol == 2: scopes.add(scope)
            report['owners'].append({'pid': int(process.name), 'executable': executable, 'scope': scope,
                                     'protocol': protocol,
                                     'starttime': (process / 'stat').read_text().rsplit(')', 1)[1].split()[19],
                                     'modules': [line for line in (process / 'maps').read_text().splitlines()
                                                 if '/worminal/' in line or 'frontend' in line or 'browser-' in line]})
        except (FileNotFoundError, ProcessLookupError, PermissionError):
            continue
    for scope in sorted(scopes):
        try:
            with connect(scope) as peer:
                tabs = catalog(peer)
                pid = identity(peer, legacy=True)
            indices = terminal_descriptors(pid)
            terminals = {}
            for process in Path('/proc').iterdir():
                if not process.name.isdecimal(): continue
                try:
                    if process.stat().st_uid != os.getuid(): continue
                    tty = os.readlink(process / 'fd/0')
                    if tty not in {f'/dev/pts/{index}' for index in indices}: continue
                    terminals[process.name] = {'tty': tty, 'comm': (process / 'comm').read_text().strip(),
                        'starttime': (process / 'stat').read_text().rsplit(')', 1)[1].split()[19]}
                except (FileNotFoundError, ProcessLookupError, PermissionError):
                    continue
            report['workspaces'][scope] = {'pid': pid, 'tabs': tabs, 'pty_indices': indices, 'processes': terminals}
        except OSError as error:
            report['workspaces'][scope] = {'unavailable': str(error)}
    names = subprocess.run(['busctl', '--user', '--no-legend', '--no-pager', 'list'],
                           capture_output=True, text=True, timeout=5)
    contract = {name: json.loads(value) for name, value in re.findall(
        r'^#define WORMINAL_STATE_(PATH|INTERFACE|PREFIX|METHOD) ("[^"]*")$',
        (ROOT / 'src/gtk/window_state_protocol.h').read_text(), re.MULTILINE)}
    if names.returncode:
        report['window_bus_error'] = names.stderr.strip()
    else:
        for line in names.stdout.splitlines():
            name = line.split()[0]
            if not name.startswith(contract['PREFIX']): continue
            result = subprocess.run(['busctl', '--user', '--json=short', 'call', name,
                contract['PATH'], contract['INTERFACE'], contract['METHOD']],
                capture_output=True, text=True, timeout=5)
            report['windows'][name] = json.loads(result.stdout) if not result.returncode else result.stderr.strip()
    (directory / 'inventory.json').write_text(json.dumps(report, indent=2) + '\n')
    print(f"Cutover recovery/inventory saved: {directory.relative_to(ROOT)}")
    print(f"Preserved {len(report['artifacts'])} artifacts; recorded {len(report['owners'])} GUI processes "
          f"and {len(report['workspaces'])} v2 workspace scopes. No processes changed.")
    return directory


def verify_baseline():
    directory = ROOT / ".checks/c-baseline"
    manifest = json.loads((directory / "manifest.json").read_text())
    verified = 0
    for record in manifest["artifacts"]:
        if record.get("missing"):
            continue
        artifact = directory / record["copy"]
        if hashlib.sha256(artifact.read_bytes()).hexdigest() != record["sha256"]:
            raise ValueError(f"Preserved C recovery artifact has changed: {record['copy']}")
        verified += 1
    if not verified:
        raise ValueError("Preserved C recovery baseline contains no artifacts")
    return verified


def preserve_baseline():
    destination = ROOT / ".checks/c-baseline"
    # Exclusive creation prevents a later Rust build from silently replacing the C recovery artifacts.
    destination.mkdir(parents=True, exist_ok=False)
    records = []
    try:
        for source, installed, _mode in installation_files("/usr/local"):
            for kind, artifact in (("build", source), ("installed", installed)):
                if not artifact.is_file():
                    records.append({"kind": kind, "path": str(artifact), "missing": True})
                    continue
                relative = artifact.relative_to(ROOT) if kind == "build" else artifact.relative_to("/")
                target = destination / kind / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(artifact, target)
                digest = hashlib.sha256(target.read_bytes()).hexdigest()
                records.append({"kind": kind, "path": str(artifact),
                                "copy": str(target.relative_to(destination)), "sha256": digest})
        revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
        (destination / "manifest.json").write_text(json.dumps(
            {"revision": revision, "artifacts": records}, indent=2) + "\n")
    except BaseException:
        # Incomplete copies must not masquerade as a usable recovery baseline.
        shutil.rmtree(destination)
        raise
    return destination
