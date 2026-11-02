"""Bootstrap handoff-capable pre-automatic daemons; normal upgrades run inside worminald."""
import fcntl
import os
from pathlib import Path
import socket
import signal
import subprocess
import struct
import sys
import time
import uuid

ROOT = Path(__file__).resolve().parent.parent
# The maintenance and regression clients share one wire/catalog decoder.
sys.path.insert(0, str(ROOT / 'tests'))
from workspace_protocol import catalog_records, send, string, wait_kind, service_pid


def connect(scope):
    peer = socket.socket(socket.AF_UNIX)
    peer.settimeout(8)
    try:
        peer.connect(f'\0worminal-workspace-v2-{os.getuid()}-{scope}')
    except BaseException:
        peer.close()
        raise
    return peer


def identity(peer, legacy=False):
    creator, uid, _ = struct.unpack('3i', peer.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
    if uid != os.getuid():
        raise RuntimeError('Workspace service belongs to another user.')
    timeout = peer.gettimeout()
    try:
        if legacy:
            peer.settimeout(.2)
        return service_pid(peer)
    except TimeoutError:
        if not legacy:
            raise
        return creator
    finally:
        peer.settimeout(timeout)


def catalog(peer):
    send(peer, 19, version=2)
    return catalog_records(wait_kind(peer, 7, version=2)[1])


def transfer(scope, token):
    with connect(scope) as peer:
        send(peer, 27, payload=string(token), version=2)
        # The command socket receives only its transfer result.
        from workspace_protocol import packet
        kind, _, payload = packet(peer, version=2)
        if kind != 25:
            raise RuntimeError('Live handoff rejected; donor retains its tabs.')


def stop_empty(scope, expected_pid, legacy=False):
    with connect(scope) as peer:
        if identity(peer, legacy) != expected_pid or catalog(peer):
            raise RuntimeError('Refusing to stop a changed or nonempty daemon.')
        send(peer, 13, version=2)
        while peer.recv(4096):
            pass


def receiver(executable, scope, ids, environment, retiring_pid=0):
    token = 'bootstrap-' + uuid.uuid4().hex
    listener = socket.socket(socket.AF_UNIX)
    receiving = socket.socket(socket.AF_UNIX, socket.SOCK_SEQPACKET)
    manifest = os.memfd_create('worminal-upgrade-identities', os.MFD_CLOEXEC | os.MFD_ALLOW_SEALING)
    try:
        listener.bind(f'\0worminal-workspace-v2-{os.getuid()}-{scope}')
        listener.listen(16)
        receiving.bind(f'\0worminal-handoff-{os.getuid()}-{token}')
        receiving.listen(1)
        data = [len(ids), max(ids, default=0) + 1, retiring_pid, *ids]
        os.write(manifest, struct.pack(f'={len(data)}I', *data))
        fcntl.fcntl(manifest, fcntl.F_ADD_SEALS,
                    fcntl.F_SEAL_WRITE | fcntl.F_SEAL_GROW | fcntl.F_SEAL_SHRINK | fcntl.F_SEAL_SEAL)
        copies = [fcntl.fcntl(fd, fcntl.F_DUPFD_CLOEXEC, 64)
                  for fd in (listener.fileno(), receiving.fileno(), manifest)]
        try:
            actions = [(os.POSIX_SPAWN_DUP2, fd, target) for fd, target in zip(copies, (3, 4, 5))]
            # Daemons must outlive this maintenance command without retaining its pipes.
            null = os.open(os.devnull, os.O_RDWR | os.O_CLOEXEC)
            try:
                actions += [(os.POSIX_SPAWN_DUP2, null, target) for target in (0, 1, 2)]
                child = os.posix_spawn(str(executable), [str(executable), '--handoff-receiver'],
                                       {**environment, 'WORMINAL_SHARED_SOCKET_SCOPE': scope}, file_actions=actions)
            finally:
                os.close(null)
        finally:
            for fd in copies:
                os.close(fd)
        return child, token
    finally:
        listener.close(); receiving.close(); os.close(manifest)


def wait_ready(scope, expected_pid, expected):
    deadline = time.monotonic() + 8
    while True:
        with connect(scope) as peer:
            actual_pid, rows = identity(peer), catalog(peer)
        if actual_pid != expected_pid:
            raise RuntimeError('Workspace endpoint changed unexpectedly; transferred sessions were left intact.')
        if rows == expected:
            return
        if time.monotonic() >= deadline:
            raise RuntimeError('Transferred workspace metadata differs; sessions were left intact.')
        time.sleep(.02)


def terminal_descriptors(pid):
    terminals = []
    for descriptor in Path(f'/proc/{pid}/fdinfo').iterdir():
        try:
            for line in descriptor.read_text().splitlines():
                if line.startswith('tty-index:'):
                    terminals.append(int(line.split(':', 1)[1]))
        except FileNotFoundError:
            pass  # Transient client sockets can close during enumeration.
    return sorted(terminals)


def upgrade(executable=Path('/usr/local/bin/worminald')):
    executable = executable.resolve(strict=True)
    scope = os.environ.get('WORMINAL_SHARED_SOCKET_SCOPE', '')
    with connect(scope) as peer:
        original, expected = identity(peer, legacy=True), catalog(peer)
    if Path(f'/proc/{original}/exe').samefile(executable):
        print('Installed daemon is already active.')
        return
    if not expected:
        stop_empty(scope, original, legacy=True)
        with open(os.devnull, 'r+b') as null:
            process = subprocess.Popen([str(executable), '--serve'], stdin=null, stdout=null, stderr=null,
                                       env=os.environ, start_new_session=True)
        deadline = time.monotonic() + 5
        while True:
            try:
                with connect(scope) as peer:
                    replacement = identity(peer)
                break
            except (ConnectionRefusedError, FileNotFoundError):
                if time.monotonic() >= deadline:
                    raise RuntimeError('Replacement did not start; old workspace was empty.')
                time.sleep(.01)
        print(f'Daemon {replacement} is active; no live tabs needed transferring.')
        return replacement
    ids = [row[0] for row in expected]
    terminals = terminal_descriptors(original)
    # The donor can gain/lose tabs between inspection and transfer. The native
    # receiver validates the mapping count before commit; mismatch preserves the donor.
    staging_scope = 'upgrade-' + uuid.uuid4().hex
    staging, token = receiver(executable, staging_scope, ids, os.environ, original)
    print(f'Transferring {len(ids)} live tabs from daemon {original}.', flush=True)
    try:
        # C has committed before the receiver pauses the old donor, which is
        # still waiting for A. Do not wait for that donor's command reply.
        donor_handle = os.pidfd_open(original)
        try:
            with connect(scope) as donor:
                send(donor, 27, payload=string(token), version=2)
                wait_ready(staging_scope, staging, expected)
                if terminal_descriptors(staging) != terminals:
                    raise RuntimeError('Transferred PTYs differ; retain the stopped donor and receiver for recovery.')
                # Only a verified receiver permits retiring the stopped donor.
                # SIGKILL closes its duplicate FDs without running shutdown hooks
                # that might hang up PTYs now owned by the replacement.
                signal.pidfd_send_signal(donor_handle, signal.SIGKILL)
                while donor.recv(4096):
                    pass
        finally:
            os.close(donor_handle)
        deadline = time.monotonic() + 5
        while True:
            try:
                replacement, token = receiver(executable, scope, ids, os.environ)
                break
            except OSError as error:
                import errno
                if error.errno != errno.EADDRINUSE or time.monotonic() >= deadline:
                    raise
                time.sleep(.01)
        transfer(staging_scope, token)
        wait_ready(scope, replacement, expected)
        if terminal_descriptors(replacement) != terminals:
            raise RuntimeError('Replacement PTYs differ; keep both receivers for recovery.')
        stop_empty(staging_scope, staging)
        os.waitpid(staging, 0)
        if Path(f'/proc/{replacement}/exe').read_bytes() != executable.read_bytes():
            raise RuntimeError('The active executable does not match the installed build.')
        print(f'Daemon {replacement} now owns all {len(ids)} tabs; automatic upgrades are active.')
        return replacement
    except BaseException:
        # Never terminate a receiver after a possible commit. Its scope is a
        # recovery destination, even if verification or the second hop failed.
        print(f'Upgrade interrupted. Preserve staging workspace {staging_scope} (pid {staging}).', file=sys.stderr)
        print('Rollback code is available in Git; never restart live shells for recovery.', file=sys.stderr)
        raise
