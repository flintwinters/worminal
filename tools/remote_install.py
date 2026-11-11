"""Native install-on-remote bootstrap. Embedded in worminald; uses only Python's standard library."""
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import stat
import subprocess
import sys
import tempfile

PREFIX = Path('/usr/local')
LIMIT = 64 * 1024 * 1024
HEADER_LIMIT = 16384


def message(value):
    print(json.dumps(value), flush=True)


def header(stream):
    line = stream.readline(HEADER_LIMIT + 1)
    if len(line) > HEADER_LIMIT or not line.endswith(b'\n'):
        raise ValueError('Invalid or incomplete installation header.')
    value = json.loads(line)
    if not isinstance(value, dict):
        raise ValueError('Invalid installation header.')
    return value


def exact(stream, size):
    remaining = size
    while remaining:
        data = stream.read(min(65536, remaining))
        if not data:
            raise ValueError('Transfer interrupted; no complete installation was published.')
        remaining -= len(data)
        yield data


def destinations(layout):
    bases = {'prefix': PREFIX, 'man': PREFIX / layout['man_dir']}
    return [(item, bases[item['base']] / item['path']) for item in layout['files']]


def active_workspace():
    # Detect both service generations without sending commands or starting a daemon.
    prefixes = (f'@worminal-workspace-{os.getuid()}-', f'@worminal-workspace-v2-{os.getuid()}-')
    for line in Path('/proc/net/unix').read_text().splitlines():
        for prefix in prefixes:
            if prefix in line:
                return True
    return False


def refuse_existing(layout):
    if shutil.which('worminald') or shutil.which('worminal'):
        raise ValueError('Worminal is already installed; use its explicit upgrade workflow. Nothing was replaced.')
    if any(os.path.lexists(path) for _, path in destinations(layout)):
        raise ValueError('An installation destination already exists. Nothing was replaced.')
    if active_workspace():
        raise ValueError('An existing Worminal workspace is active. Nothing was replaced or stopped.')


def legacy_workspace():
    # A v1 workspace must never be disturbed; its prefix lacks the v2 marker.
    prefix = f'@worminal-workspace-{os.getuid()}-'
    return any(prefix in line for line in Path('/proc/net/unix').read_text().splitlines())


def installed_version(layout):
    """Version reported by the managed daemon; None when it predates `--version` or cannot run."""
    daemon = next(path for item, path in destinations(layout) if item['role'] == 'daemon')
    if not stat.S_ISREG(os.lstat(daemon).st_mode):
        raise ValueError('The installed daemon is not a regular file; nothing was replaced.')
    try:
        result = subprocess.run([str(daemon), '--version'], capture_output=True, text=True, timeout=10)
    except (OSError, subprocess.TimeoutExpired):
        return None
    return result.stdout.strip() if result.returncode == 0 and result.stdout.strip() else None


def payload(layout, stream, value=None):
    value = value or header(stream)
    records = value.get('files')
    if not isinstance(records, list) or len(records) != len(layout['files']):
        raise ValueError('Incomplete installation payload.')
    for expected, record in zip(layout['files'], records):
        if record.get('role') != expected['role'] or type(record.get('size')) is not int:
            raise ValueError('Invalid installation payload record.')
        if not 0 < record['size'] <= LIMIT:
            raise ValueError('Installation payload exceeds the size limit.')
    return records


def private_parent(path):
    # PREFIX is fixed, not supplied by the remote request. Walk its descendants with
    # directory descriptors so privileged publication cannot follow user-created symlinks.
    try:
        os.mkdir(PREFIX, 0o755)
    except FileExistsError:
        pass
    fd = os.open(PREFIX, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        info = os.fstat(fd)
        if info.st_uid != os.geteuid() or info.st_mode & 0o022:
            raise ValueError('Installation prefix is writable by another user; refusing publication.')
        for component in path.parent.relative_to(PREFIX).parts:
            try:
                os.mkdir(component, 0o755, dir_fd=fd)
            except FileExistsError:
                pass
            next_fd = os.open(component, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
            os.close(fd)
            fd = next_fd
            info = os.fstat(fd)
            if info.st_uid != os.geteuid() or info.st_mode & 0o022:
                raise ValueError('Installation directory is writable by another user; refusing publication.')
        return fd
    except BaseException:
        os.close(fd)
        raise


def publish(layout, stream, replace=False):
    records = payload(layout, stream)
    staged, published = [], []
    try:
        # Preflight every destination before writing or publishing any payload file.
        parents = []
        try:
            for item, path in destinations(layout):
                fd = private_parent(path)
                parents.append((item, path, fd))
                try:
                    info = os.stat(path.name, dir_fd=fd, follow_symlinks=False)
                except FileNotFoundError:
                    if replace and item['role'] == 'daemon':
                        raise ValueError('Worminal is not installed here. Nothing was replaced.')
                    continue
                if replace and stat.S_ISREG(info.st_mode):
                    continue
                raise ValueError('Installation destination appeared concurrently. Nothing was replaced.')
            for record, (item, path, fd) in zip(records, parents):
                name = '.worminal-install-' + os.urandom(12).hex()
                handle = os.open(name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                                 0o600, dir_fd=fd)
                staged.append((fd, name))
                digest = hashlib.sha256()
                with os.fdopen(handle, 'wb') as output:
                    for data in exact(stream, record['size']):
                        digest.update(data)
                        output.write(data)
                    output.flush()
                    os.fchmod(output.fileno(), item['mode'])
                    os.fsync(output.fileno())
                if digest.hexdigest() != record['sha256']:
                    raise ValueError('Payload checksum changed; no installation was published.')
            if replace:
                # rename swaps the inode atomically, so the running daemon keeps its mapped code and its
                # executable watch hands live shells to the new file. The daemon goes last: an interruption
                # can leave a newer manual, never a half-written daemon.
                for (fd, name), (_, path, _) in reversed(list(zip(staged, parents))):
                    os.rename(name, path.name, src_dir_fd=fd, dst_dir_fd=fd)
                    os.fsync(fd)
            else:
                for (fd, name), (_, path, _) in zip(staged, parents):
                    # link is atomic and refuses replacement, including a concurrent symlink.
                    os.link(name, path.name, src_dir_fd=fd, dst_dir_fd=fd, follow_symlinks=False)
                    published.append((fd, path.name, os.stat(name, dir_fd=fd, follow_symlinks=False).st_ino))
                    os.fsync(fd)
        finally:
            for fd, name in staged:
                try:
                    os.unlink(name, dir_fd=fd)
                except FileNotFoundError:
                    pass
            if sys.exc_info()[0]:
                for fd, name, inode in published:
                    try:
                        if os.stat(name, dir_fd=fd, follow_symlinks=False).st_ino == inode:
                            os.unlink(name, dir_fd=fd)
                    except FileNotFoundError:
                        pass
            for _, _, fd in parents:
                os.close(fd)
    except BaseException:
        raise


def root_command(layout, source, replace):
    code = f'LAYOUT={layout!r}\nMODE={"upgrade" if replace else "install"!r}\nSOURCE={source!r}\n' + source
    command = [sys.executable, '-c', code, '--publish']
    return command if os.geteuid() == 0 else ['sudo', '-n', *command]


def send(layout):
    """Stream the installed files to an out-of-date peer that asked for them (the pull direction)."""
    files = []
    try:
        for item, path in destinations(layout):
            handle = open(path, 'rb')
            files.append((item['role'], handle, os.fstat(handle.fileno()).st_size))
        message({'files': [{'role': role, 'size': size} for role, _, size in files]})
        for _, handle, size in files:
            for data in exact(handle, size):
                sys.stdout.buffer.write(data)
        sys.stdout.buffer.flush()
    finally:
        for _, handle, _ in files:
            handle.close()
    return {'ok': True, 'sent': True}


def bootstrap(layout, source, replace=False):
    ready = {'ready': True, 'system': platform.system(), 'machine': platform.machine()}
    first = None
    if replace:
        ready['version'] = installed_version(layout)
    message(ready)
    if replace:
        # The controller compares versions: cancel (same), send (this node is newer) or files (older).
        first = header(sys.stdin.buffer)
        if first.get('cancel'):
            return {'ok': True, 'installed': False, 'current': True}
        if first.get('send'):
            return send(layout)
        if legacy_workspace():
            raise ValueError('A v1 Worminal workspace is active; it is never upgraded implicitly. Nothing was replaced.')
    records = payload(layout, sys.stdin.buffer, first)
    if not replace:
        refuse_existing(layout)
    if os.geteuid() != 0:
        if subprocess.run(['sudo', '-n', 'true'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode:
            raise ValueError('Non-interactive sudo is required on the target; nothing was installed.')
    message({'accept': True})
    cache = Path(os.environ.get('XDG_CACHE_HOME', Path.home() / '.cache')) / 'worminal'
    cache.mkdir(mode=0o700, parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='install-', dir=cache) as directory:
        paths = []
        for record in records:
            path = Path(directory) / record['role']
            digest = hashlib.sha256()
            with path.open('xb') as output:
                for data in exact(sys.stdin.buffer, record['size']):
                    digest.update(data)
                    output.write(data)
            path.chmod(0o700 if record['role'] == 'daemon' else 0o600)
            record['sha256'] = digest.hexdigest()
            paths.append(path)
        candidate = paths[[item['role'] for item in layout['files']].index('daemon')]
        try:
            result = subprocess.run([str(candidate), 'api', 'help'], capture_output=True, timeout=10)
        except (OSError, subprocess.TimeoutExpired) as error:
            raise ValueError('The staged daemon cannot run here (runtime/libc incompatibility). '
                             'Nothing was installed.') from error
        if result.returncode or b'Worp' not in result.stdout:
            raise ValueError('The staged daemon cannot run here (runtime/libc incompatibility). Nothing was installed.')
        if not replace:
            refuse_existing(layout)
        # The privileged publisher consumes bytes from stdin, never user-controlled source paths.
        process = subprocess.Popen(root_command(layout, source, replace), stdin=subprocess.PIPE,
                                   stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        try:
            process.stdin.write((json.dumps({'files': records})+'\n').encode())
            for path in paths:
                with path.open('rb') as input_file:
                    shutil.copyfileobj(input_file, process.stdin, 65536)
            process.stdin.close()
            output = process.stdout.read(HEADER_LIMIT + 1)
            if process.wait(timeout=30):
                raise ValueError('Privileged publication failed; existing destinations were not replaced.')
            result = json.loads(output)
            if not result.get('ok'):
                raise ValueError(result.get('error', 'Privileged publication failed.'))
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
    return {'ok': True, 'installed': True, 'upgraded': replace, 'started': False, 'prefix': str(PREFIX),
            'files': [{'role': row['role'], 'sha256': row['sha256']} for row in records]}


def main(layout, source):
    try:
        replace = globals().get('MODE', 'install') == 'upgrade'
        if sys.argv[-1] == '--publish':
            publish(layout, sys.stdin.buffer, replace)
            message({'ok': True})
        else:
            message(bootstrap(layout, source, replace))
    except Exception as error:
        # No tracebacks, credential files, environment dumps or subprocess output in responses.
        detail = str(error) if isinstance(error, ValueError) else f'Installation failed ({type(error).__name__}).'
        message({'ok': False, 'error': detail})
        raise SystemExit(1)


if __name__ == '__main__':
    main(LAYOUT, SOURCE)
