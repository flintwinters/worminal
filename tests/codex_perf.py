"""Type an unsubmitted draft in real Codex, through ordinary SSH and --master."""

import json
import os
from pathlib import Path
import select
import shlex
import struct
import subprocess
import sys
import time
import uuid

from master_perf import SSH, launch, receive, send, summary

ROOT = Path(__file__).resolve().parent.parent
CHECKS = ROOT / '.checks'
BURST_INTERVAL = .05


class View:
    def __init__(self):
        self.cells = [[' '] * 120 for _ in range(40)]
        self.cursor = (0, 0)
        self.counts = dict(packets=0, bytes=0, frames=0, rows=0, catalogs=0, copies=0)

    def apply(self, kind, identity, payload, size, tab):
        self.counts['packets'] += 1
        self.counts['bytes'] += size
        if kind == 7:
            self.counts['catalogs'] += 1
        if identity != tab:
            return False
        if kind == 8:
            cols, rows, x, y, _, _ = struct.unpack('!6I', payload)
            if len(self.cells) != rows or len(self.cells[0]) != cols:
                self.cells = [[' '] * cols for _ in range(rows)]
            self.cursor = x, y
        elif kind == 9:
            row, = struct.unpack_from('!I', payload)
            self.cells[row] = [chr(struct.unpack_from('!I', payload, offset)[0] or 32)
                               for offset in range(4, len(payload), 16)]
            self.counts['rows'] += 1
        elif kind == 29:
            source, destination, count = struct.unpack('!3I', payload)
            self.cells[destination:destination + count] = [r[:] for r in self.cells[source:source + count]]
            self.counts['copies'] += 1
        elif kind == 10:
            self.counts['frames'] += 1
            return True
        return False

    def anchor(self):
        x, y = self.cursor
        prefix = ''.join(self.cells[y][:x]).strip(' │┃')
        return (x, y) if prefix in ('›', '>', '❯', '❱') else None

    def save(self, label):
        (CHECKS / f'codex-perf-{label}-screen.txt').write_text(
            '\n'.join(''.join(row).rstrip() for row in self.cells) + '\n')


def ready(process, view, tab, label):
    deadline = time.monotonic() + 60
    last_frame = time.monotonic()
    while time.monotonic() < deadline:
        if not select.select([process.stdout], [], [], .2)[0]:
            if view.anchor() and time.monotonic() - last_frame >= .5:
                return view.anchor()
            continue
        packet = receive(process, deadline)
        if view.apply(*packet, tab):
            last_frame = time.monotonic()
    view.save(label)
    raise RuntimeError(f'Codex input editor not ready; inspect .checks/codex-perf-{label}-screen.txt')


def wait_text(process, view, tab, anchor, text):
    x, y = anchor
    deadline = time.monotonic() + 20
    while True:
        complete = view.apply(*receive(process, deadline), tab)
        if complete and ''.join(view.cells[y][x:x + len(text)]) == text:
            return


def burst(process, view, tab, anchor, initial):
    x, y = anchor
    text = ' with a burst of unsubmitted typing to measure actual keystroke pacing'
    text = text[:len(view.cells[y]) - x - len(initial) - 1]
    expected = initial + text
    sent, samples = [], []
    started = time.monotonic()
    deadline = started + 30
    before = dict(view.counts)
    while len(samples) < len(text):
        now = time.monotonic()
        if now >= deadline:
            raise TimeoutError('Codex did not display the complete unsubmitted burst')
        while len(sent) < len(text) and now >= started + BURST_INTERVAL * len(sent):
            letter = text[len(sent)]
            sent.append(time.monotonic_ns())
            send(process, 3, tab, letter.encode())
        delay = .1
        if len(sent) < len(text):
            delay = min(delay, max(started + BURST_INTERVAL * len(sent) - time.monotonic(), 0))
        if not select.select([process.stdout], [], [], delay)[0]:
            continue
        if view.apply(*receive(process, deadline), tab):
            shown = ''.join(view.cells[y][x:])
            matched = 0
            for actual, wanted in zip(shown, expected):
                if actual != wanted:
                    break
                matched += 1
            visible = min(max(matched - len(initial), 0), len(sent))
            observed = time.monotonic_ns()
            while len(samples) < visible:
                samples.append((observed - sent[len(samples)]) / 1e6)
    return samples, {name: view.counts[name] - before[name] for name in before}


def measure(host, remote_command, remote):
    label = 'master' if remote else 'ordinary-ssh'
    scope = 'codex-perf-' + uuid.uuid4().hex
    daemon = os.environ.get('WORMINAL_MASTER_DAEMON', 'worminald')
    prefix = f'env WORMINAL_SHARED_SOCKET_SCOPE={shlex.quote(scope)} {shlex.quote(daemon)}'
    env = {**os.environ, 'WORMINAL_SHARED_SOCKET_SCOPE': scope}
    local_daemon = os.environ.get('WORMINAL_LOCAL_DAEMON', str(ROOT / 'worminald'))
    arguments = [*SSH, '-C', host, f'{prefix} --bridge'] if remote else [local_daemon, '--bridge']
    stop = [*SSH, host, f'{prefix} --stop'] if remote else [local_daemon, '--stop']
    log = (CHECKS / f'codex-perf-{label}.log').open('w+b')
    process = subprocess.Popen(arguments, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log)
    tab = None
    try:
        send(process, 28, payload=struct.pack('!I', 1))
        command = ['/bin/sh', '-c', remote_command] if remote else [*SSH, '-tt', host, remote_command]
        tab, _, _ = launch(process, 1, command)
        view = View()
        x, y = ready(process, view, tab, label)
        # Remove first-edit placeholder/layout changes from steady typing samples.
        send(process, 3, tab, b'~')
        wait_text(process, view, tab, (x, y), '~')
        send(process, 3, tab, b'\x7f')
        x, y = ready(process, view, tab, label)
        samples, counts, expected = [], [], ''
        for letter in 'worminal-latency-probe-0123456789':
            expected += letter
            before = dict(view.counts)
            started = time.monotonic_ns()
            send(process, 3, tab, letter.encode())
            wait_text(process, view, tab, (x, y), expected)
            samples.append((time.monotonic_ns() - started) / 1e6)
            counts.append({name: view.counts[name] - before[name] for name in before})
        view.save(label)
        summary(f'Real Codex typing via {label}', samples)
        print(f'{label}: {sum(c["frames"] for c in counts)} frames, '
              f'{sum(c["catalogs"] for c in counts)} catalogs, '
              f'{sum(c["bytes"] for c in counts)} protocol bytes; draft NOT submitted', flush=True)
        burst_samples, burst_counts = burst(process, view, tab, (x, y), expected)
        view.save(label)
        summary(f'Real Codex continuous typing via {label}', burst_samples)
        print(f'{label} burst protocol counts: {burst_counts}', flush=True)
        return dict(samples_ms=samples, packets_per_key=counts,
                    burst_samples_ms=burst_samples, burst_counts=burst_counts, burst_interval_ms=BURST_INTERVAL * 1000)
    finally:
        if tab is not None:
            try:
                send(process, 5, tab)
            except (BrokenPipeError, OSError):
                pass
        subprocess.run(stop, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=20)
        process.terminate()
        try:
            process.communicate(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.communicate()
        log.close()


def main():
    if len(sys.argv) != 2:
        raise SystemExit('Usage: python3 manage.py codex-perf HOST; WORMINAL_MASTER_CWD selects remote project')
    CHECKS.mkdir(exist_ok=True)
    directory = os.environ.get('WORMINAL_MASTER_CWD')
    command = (f'cd {shlex.quote(directory)} && ' if directory else '') + 'exec codex'
    results = {}
    for remote in (False, True):
        label = 'master' if remote else 'ordinary-ssh'
        results[label] = measure(sys.argv[1], command, remote)
        (CHECKS / 'codex-perf.json').write_text(json.dumps(results, indent=2) + '\n')


if __name__ == '__main__':
    main()
