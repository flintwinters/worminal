"""SSH-side upgrade transaction. Standalone when embedded by remote_upgrade.py; standard library only."""
import ctypes
import fcntl
import hashlib
import json
import os
from pathlib import Path
import platform
import stat
import subprocess
import sys
import tempfile
import time

if 'SUPPORT_SOURCE' in globals():
    support = {'__name__': 'worminal_remote_install'}
    exec(SUPPORT_SOURCE, support)
else:
    from tools import remote_install
    support = vars(remote_install)

PREFIX = Path('/usr/local')
LIMIT = 64 * 1024 * 1024


def digest(path):
    with os.fdopen(os.open(path, os.O_RDONLY | os.O_NOFOLLOW), 'rb') as source:
        info = os.fstat(source.fileno())
        if not stat.S_ISREG(info.st_mode):
            raise ValueError('An installation destination is not a regular file.')
        result = hashlib.file_digest(source, 'sha256').hexdigest()
    return result


def validate(layout):
    rows = layout.get('files', [])
    if not isinstance(layout.get('graphics'), bool) or not 2 <= len(rows) <= 16:
        raise ValueError('Invalid upgrade manifest.')
    names = set()
    for row in rows:
        path = Path(row['role'])
        if path.is_absolute() or '..' in path.parts or str(path) in names or row['mode'] not in (0o644, 0o755):
            raise ValueError('Invalid upgrade destination.')
        if not isinstance(row['size'], int) or not 0 < row['size'] <= LIMIT:
            raise ValueError('Upgrade artifact exceeds the size bound.')
        if len(row['sha256']) != 64 or any(value not in '0123456789abcdef' for value in row['sha256']):
            raise ValueError('Invalid artifact checksum.')
        names.add(str(path))
    if 'bin/worminald' not in names or bool('bin/worminal' in names) != layout['graphics']:
        raise ValueError('Incomplete installation layout.')
    if sum(row['size'] for row in rows) > 256 * 1024 * 1024:
        raise ValueError('Upgrade transfer exceeds the size bound.')


def preflight(layout):
    validate(layout)
    if not (PREFIX / 'bin/worminald').is_file():
        raise ValueError('Target has no existing installation; use install-on-remote first.')
    if (PREFIX / 'bin/worminal').exists() != layout['graphics']:
        raise ValueError('Source and target must both be headless or both desktop installations.')
    if os.geteuid() != 0 and subprocess.run(['sudo', '-n', 'true'], stdout=subprocess.DEVNULL,
                                          stderr=subprocess.DEVNULL).returncode:
        raise ValueError('Non-interactive sudo is required; nothing was replaced.')
    for row in layout['files']:
        row['previous'] = digest(PREFIX / row['role'])
        row['previous_mode'] = (PREFIX / row['role']).lstat().st_mode & 0o777


def exchange(parent, left, right):
    libc = ctypes.CDLL(None, use_errno=True)
    rename = getattr(libc, 'renameat2', None)
    if rename is None or rename(parent, os.fsencode(left), parent, os.fsencode(right), 2):
        raise OSError(ctypes.get_errno(), 'Atomic upgrade exchange is unavailable')


def publish(layout, stream):
    """Exchange every staged inode; restore original inodes if publication fails before completion."""
    validate(layout)
    support['PREFIX'] = PREFIX
    parents, staged, exchanged = [], [], []
    cleanup = True
    lock_parent = support['private_parent'](PREFIX / '.worminal-upgrade.lock')
    lock = os.open('.worminal-upgrade.lock', os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600, dir_fd=lock_parent)
    os.close(lock_parent)
    try:
        info = os.fstat(lock)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.geteuid() or info.st_mode & 0o077:
            raise ValueError('Unsafe upgrade lock; nothing was replaced.')
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        for row in layout['files']:
            path = PREFIX / row['role']
            parent = support['private_parent'](path)
            parents.append(parent)
            fd = os.open(path.name, os.O_RDONLY | os.O_NOFOLLOW, dir_fd=parent)
            with os.fdopen(fd, 'rb') as previous:
                identity = os.fstat(previous.fileno())
                if not stat.S_ISREG(identity.st_mode) or identity.st_uid != os.geteuid():
                    raise ValueError('Unsafe existing installation ownership/type.')
                if hashlib.file_digest(previous, 'sha256').hexdigest() != row.get('previous'):
                    raise ValueError('Target changed since preflight; nothing was replaced.')
            temporary = '.worminal-upgrade-' + os.urandom(12).hex()
            fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=parent)
            staged.append((parent, temporary, path.name, identity.st_ino, row))
            with os.fdopen(fd, 'wb') as candidate:
                checksum = hashlib.sha256()
                for data in support['exact'](stream, row['size']):
                    candidate.write(data)
                    checksum.update(data)
                candidate.flush()
                os.fchmod(candidate.fileno(), row['mode'])
                os.fsync(candidate.fileno())
            if checksum.hexdigest() != row['sha256']:
                raise ValueError('Upgrade checksum failed; nothing was replaced.')
        # Every byte is validated before any installed pathname changes.
        for parent, temporary, name, previous_inode, row in staged:
            if row['previous'] == row['sha256'] and row.get('previous_mode') == row['mode']:
                continue
            if os.stat(name, dir_fd=parent, follow_symlinks=False).st_ino != previous_inode:
                raise ValueError('Concurrent installer changed a destination.')
            candidate_inode = os.stat(temporary, dir_fd=parent, follow_symlinks=False).st_ino
            exchange(parent, temporary, name)
            exchanged.append((parent, temporary, name, candidate_inode))
            if os.stat(temporary, dir_fd=parent, follow_symlinks=False).st_ino != previous_inode:
                raise ValueError('Concurrent installer changed a destination during publication.')
            os.fsync(parent)
    except BaseException:
        try:
            for parent, temporary, name, candidate_inode in reversed(exchanged):
                if os.stat(name, dir_fd=parent, follow_symlinks=False).st_ino != candidate_inode:
                    raise RuntimeError('Concurrent change prevents rollback; '
                                       'preserve transaction files and inspect target.')
                exchange(parent, temporary, name)
                os.fsync(parent)
        except BaseException:
            cleanup = False
            raise
        raise
    finally:
        if cleanup:
            for parent, temporary, _, _, _ in staged:
                try:
                    os.unlink(temporary, dir_fd=parent)
                except FileNotFoundError:
                    pass
        for parent in parents:
            os.close(parent)
        os.close(lock)


def owners():
    """Read current account's installed process identities; never signal or start a service."""
    result = []
    for process in Path('/proc').iterdir():
        if not process.name.isdecimal():
            continue
        try:
            if process.stat().st_uid != os.getuid():
                continue
            executable = os.readlink(process / 'exe').removesuffix(' (deleted)')
            if executable not in (str(PREFIX / 'bin/worminal'), str(PREFIX / 'bin/worminald')):
                continue
            maps = (process / 'maps').read_text().splitlines()
            modules = {}
            for line in maps:
                parts = line.split()
                if len(parts) >= 6:
                    path = ' '.join(parts[5:]).removesuffix(' (deleted)')
                    if path.startswith(str(PREFIX / 'lib/worminal') + '/'):
                        modules[path] = int(parts[4])
            result.append({'pid': int(process.name), 'role': Path(executable).name,
                           'inode': (process / 'exe').stat().st_ino, 'modules': modules})
        except (OSError, ValueError):
            continue
    return result


def terminals(live):
    indexes = set()
    for owner in live:
        if owner['role'] != 'worminald':
            continue
        for fd in Path(f'/proc/{owner["pid"]}/fdinfo').glob('*'):
            try:
                for line in fd.read_text().splitlines():
                    if line.startswith('tty-index:'):
                        indexes.add('/dev/pts/' + line.split(':', 1)[1].strip())
            except OSError:
                pass
    result = {}
    for process in Path('/proc').iterdir():
        if not process.name.isdecimal():
            continue
        try:
            if process.stat().st_uid == os.getuid() and os.readlink(process / 'fd/0') in indexes:
                result[process.name] = (process / 'stat').read_text().rsplit(')', 1)[1].split()[19]
        except OSError:
            pass
    return result


def activation(before, processes, layout, timeout=30):
    counts = {role: sum(item['role'] == role for item in before) for role in ('worminal', 'worminald')}
    deadline = time.monotonic() + timeout
    while True:
        live = owners()
        pending = []
        for item in live:
            if item['inode'] != (PREFIX / 'bin' / item['role']).stat().st_ino:
                pending.append(item['pid'])
                continue
            if item['role'] == 'worminal':
                frontend = [row for row in layout['files'] if row['role'].startswith('lib/worminal/frontend')]
                if not frontend or not any(inode == (PREFIX / frontend[0]['role']).stat().st_ino
                                           for path, inode in item['modules'].items() if 'frontend' in path):
                    pending.append(item['pid'])
                for row in layout['files']:
                    path = str(PREFIX / row['role'])
                    if path in item['modules'] and item['modules'][path] != Path(path).stat().st_ino:
                        pending.append(item['pid'])
        pending = sorted(set(pending))
        missing_roles = [role for role, count in counts.items() if sum(item['role'] == role for item in live) < count]
        if not pending and not missing_roles or time.monotonic() >= deadline:
            break
        time.sleep(.1)
    missing = []
    for pid, started in processes.items():
        try:
            if Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()[19] != started:
                missing.append(int(pid))
        except FileNotFoundError:
            missing.append(int(pid))
    return {'active': not pending and not missing_roles and not missing, 'pending_owner_pids': pending,
            'missing_owner_roles': missing_roles, 'session_processes_missing': missing,
            'preserved_session_processes': len(processes) - len(missing)}


def check_candidates(paths, layout):
    daemon = paths['bin/worminald']
    result = subprocess.run([str(daemon), 'api', 'help'], capture_output=True, timeout=10)
    if result.returncode or b'Worp' not in result.stdout:
        raise ValueError('Staged daemon cannot run on target; nothing was replaced.')
    for row in layout['files']:
        if row['role'].startswith('lib/worminal/'):
            probe = 'import ctypes,os,sys;ctypes.CDLL(sys.argv[1],mode=os.RTLD_NOW|os.RTLD_LOCAL)'
            result = subprocess.run([sys.executable, '-c', probe, str(paths[row['role']])],
                                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=10)
            if result.returncode:
                raise ValueError('Staged module dependencies are incompatible; nothing was replaced.')
    if layout['graphics']:
        result = subprocess.run([str(paths['bin/worminal']), '-v'], capture_output=True, timeout=10)
        if result.returncode:
            raise ValueError('Staged launcher cannot run on target; nothing was replaced.')


def bootstrap(layout, code):
    preflight(layout)
    before = owners()
    processes = terminals(before)
    print(json.dumps({'ready': True, 'system': platform.system(), 'machine': platform.machine()}), flush=True)
    cache = Path(os.environ.get('XDG_CACHE_HOME', Path.home() / '.cache')) / 'worminal'
    cache.mkdir(mode=0o700, parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='upgrade-', dir=cache) as directory:
        paths = {}
        for index, row in enumerate(layout['files']):
            path = Path(directory) / str(index)
            with path.open('xb') as output:
                for data in support['exact'](sys.stdin.buffer, row['size']):
                    output.write(data)
            path.chmod(row['mode'])
            if digest(path) != row['sha256']:
                raise ValueError('Transfer checksum failed; nothing was replaced.')
            paths[row['role']] = path
        check_candidates(paths, layout)
        changed = any(row['previous'] != row['sha256'] or row['previous_mode'] != row['mode']
                      for row in layout['files'])
        if changed:
            command = [sys.executable, '-c', code, '--publish']
            if os.geteuid() != 0:
                command = ['sudo', '-n', *command]
            with tempfile.TemporaryFile(dir=directory) as bundle:
                bundle.write((json.dumps(layout) + '\n').encode())
                for row in layout['files']:
                    with paths[row['role']].open('rb') as data:
                        while chunk := data.read(65536):
                            bundle.write(chunk)
                bundle.seek(0)
                child = subprocess.Popen(command, stdin=bundle, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
                try:
                    try:
                        response, _ = child.communicate(timeout=60)
                    except subprocess.TimeoutExpired as error:
                        raise RuntimeError('Publication timed out; outcome is uncertain. '
                                           'Inspect target before retrying.') from error
                    if child.returncode:
                        raise ValueError('Publication failed; transaction rollback was attempted. '
                                         'Inspect target before retrying.')
                finally:
                    if child.poll() is None:
                        child.kill()
                        child.wait()
    for row in layout['files']:
        if digest(PREFIX / row['role']) != row['sha256']:
            raise ValueError('Installed checksum changed after publication; inspect target before retrying.')
    result = activation(before, processes, layout)
    return {'ok': True, 'installed': True, 'changed': changed, 'started': False, 'prefix': str(PREFIX),
            'files': [{'path': row['role'], 'sha256': row['sha256']} for row in layout['files']], **result}


def main():
    try:
        if sys.argv[-1] == '--publish':
            layout = support['header'](sys.stdin.buffer)
            keys = ('role', 'size', 'sha256', 'mode')
            actual = [{key: row[key] for key in keys} for row in layout['files']]
            expected = [{key: row[key] for key in keys} for row in LAYOUT['files']]
            if actual != expected:
                raise ValueError('Privileged manifest differs from requested release.')
            publish(layout, sys.stdin.buffer)
            print(json.dumps({'ok': True}), flush=True)
        else:
            code = (f'LAYOUT={LAYOUT!r}\nSUPPORT_SOURCE={SUPPORT_SOURCE!r}\n'
                    f'AGENT_SOURCE={AGENT_SOURCE!r}\nexec(AGENT_SOURCE)')
            print(json.dumps(bootstrap(LAYOUT, code)), flush=True)
    except Exception as error:
        detail = (str(error) if isinstance(error, (ValueError, RuntimeError))
                  else f'Upgrade failed ({type(error).__name__}).')
        print(json.dumps({'ok': False, 'error': detail}), flush=True)
        raise SystemExit(1)


if __name__ == '__main__':
    main()
