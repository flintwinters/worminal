"""Transfer the current installation, never a repository build or a deleted running inode."""
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import select
import shlex
import subprocess
import time

from tools import build

LIMIT = 64 * 1024 * 1024
TOTAL_LIMIT = 256 * 1024 * 1024
ROOT = Path(__file__).resolve().parent.parent


def snapshot(prefix):
    prefix = Path(prefix).resolve(strict=True)
    graphics = (prefix / 'bin/worminal').exists()
    rows, data, identities = [], [], []
    for _, path, mode in build.installation_files(prefix, headless_only=not graphics):
        relative = str(path.relative_to(prefix))
        with os.fdopen(os.open(path, os.O_RDONLY | os.O_NOFOLLOW), 'rb') as source:
            before = os.fstat(source.fileno())
            if not 0 < before.st_size <= LIMIT:
                raise ValueError('Installed source file exceeds the 64 MiB bound.')
            payload = source.read(LIMIT + 1)
            after = os.fstat(source.fileno())
        if (before.st_ino, before.st_size, before.st_mtime_ns) != (after.st_ino, after.st_size, after.st_mtime_ns):
            raise ValueError('Installed source changed during capture; retry after installation completes.')
        rows.append({'role': relative, 'size': len(payload),
                     'sha256': hashlib.sha256(payload).hexdigest(), 'mode': mode})
        data.append(payload)
        identities.append((path, before.st_dev, before.st_ino, before.st_mtime_ns))
    if sum(map(len, data)) > TOTAL_LIMIT:
        raise ValueError('Installed source exceeds the 256 MiB transfer bound.')
    for path, device, inode, modified in identities:
        current = path.stat()
        if (current.st_dev, current.st_ino, current.st_mtime_ns) != (device, inode, modified):
            raise ValueError('Source installation changed during capture; retry after installation completes.')
    return {'graphics': graphics, 'files': rows}, data


def bootstrap_code(layout):
    support = (ROOT / 'tools/remote_install.py').read_text()
    agent = (ROOT / 'tools/remote_upgrade_agent.py').read_text()
    return f'LAYOUT={layout!r}\nSUPPORT_SOURCE={support!r}\nAGENT_SOURCE={agent!r}\nexec(AGENT_SOURCE)\n'


def response(stream, timeout):
    deadline = time.monotonic() + timeout
    line = bytearray()
    while len(line) <= 16384:
        remaining = deadline - time.monotonic()
        if remaining <= 0 or not select.select([stream], [], [], remaining)[0]:
            raise RuntimeError('SSH upgrade preflight timed out; nothing was sent.')
        byte = os.read(stream.fileno(), 1)
        if not byte:
            raise RuntimeError('SSH preflight closed; verify Python 3 and SSH access. Nothing was sent.')
        line.extend(byte)
        if byte == b'\n':
            break
    if len(line) > 16384:
        raise RuntimeError('SSH upgrade response exceeds the bound.')
    value = json.loads(line)
    if not value.get('ok', True):
        raise RuntimeError(value.get('error', 'Remote preflight failed.'))
    return value


def upgrade(target, source_prefix=Path('/usr/local')):
    valid = re.fullmatch(r'[A-Za-z0-9._+\[\]:-]+(?:@[A-Za-z0-9._+\[\]:-]+)?', target)
    if not valid or len(target) > 255 or target.startswith('-') or ('@' in target and ':' in target.split('@')[0]):
        raise ValueError('Target must be an SSH host or user@host, not a URL or command.')
    layout, files = snapshot(source_prefix)
    code = bootstrap_code(layout)
    process = subprocess.Popen(['ssh', '-T', '-oBatchMode=yes', '-oConnectTimeout=10', target,
                                'python3', '-c', shlex.quote(code)], stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    sent = False
    try:
        ready = response(process.stdout, 20)
        if (ready.get('system'), ready.get('machine')) != (platform.system(), platform.machine()):
            raise ValueError('Target must have the same Linux CPU architecture as this installation; nothing was sent.')
        if platform.system() != 'Linux' or not ready.get('ready'):
            raise ValueError('Only Linux devices are supported; nothing was sent.')
        sent = True
        output, _ = process.communicate(b''.join(files), timeout=180)
        if len(output) > 65536:
            raise RuntimeError('Remote receipt exceeds the bound; inspect target before retrying.')
        try:
            receipt = json.loads(output)
        except (ValueError, UnicodeDecodeError) as error:
            raise RuntimeError('SSH reply was lost or invalid; outcome is uncertain. '
                               'Inspect target before retrying.') from error
        if process.returncode or not receipt.get('ok'):
            raise RuntimeError(receipt.get('error', 'Remote upgrade failed; inspect target before retrying.'))
        receipt['target'] = target
        receipt['source_prefix'] = str(Path(source_prefix).resolve())
        return receipt
    except subprocess.TimeoutExpired as error:
        raise RuntimeError('SSH upgrade timed out; outcome is uncertain. Inspect target before retrying.') from error
    except (BrokenPipeError, ConnectionError) as error:
        detail = 'outcome is uncertain; inspect target' if sent else 'nothing was sent'
        raise RuntimeError(f'SSH connection failed; {detail}.') from error
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
        for stream in (process.stdin, process.stdout):
            if stream and not stream.closed:
                stream.close()
