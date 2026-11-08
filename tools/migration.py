"""Read-only live-state inventories; rollback code comes from Git, never release copies."""
from tools.paths import CHECKS


import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import uuid

from tools.build import ROOT, installation_files


def inventory():
    """Record installed hashes and live state without copying releases or changing processes."""
    from tools.daemon_upgrade import connect, catalog, identity, terminal_descriptors
    directory = CHECKS / ('inventory-' + uuid.uuid4().hex)
    directory.mkdir(mode=0o700, parents=True)
    report = {'artifacts': [], 'owners': [], 'windows': {}, 'workspaces': {}}
    for _source, path, _mode in installation_files('/usr/local'):
        if not path.is_file(): continue
        report['artifacts'].append({'path': str(path), 'mode': path.stat().st_mode & 0o777,
                                    'sha256': hashlib.sha256(path.read_bytes()).hexdigest()})
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
        r'^pub const WORMINAL_STATE_(PATH|INTERFACE|PREFIX|METHOD): &str = ("[^"]*");$',
        (ROOT / 'src/frontend/src/window_state_protocol.rs').read_text(), re.MULTILINE)}
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
    print(f"Live inventory saved: {directory.relative_to(ROOT)}")
    print(f"Hashed {len(report['artifacts'])} artifacts; recorded {len(report['owners'])} GUI processes "
          f"and {len(report['workspaces'])} v2 workspace scopes. No processes changed.")
    return directory
