"""Real Codex typing through the actual GTK viewer, observed on private Xvfb pixels."""

import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import time

from PIL import Image, ImageChops
from Xlib import X, XK, display
from Xlib.ext import xtest

from master_perf import SSH, summary
from visible_latency import capture_failure, find_window, image_bytes, stop, wait_for
from x11 import isolated_display

ROOT = Path(__file__).resolve().parent.parent
CHECKS = ROOT / '.checks'
WORD = 'mwxyzabc'


def capture(window, rectangle):
    x, y, width, height = rectangle
    return image_bytes(window.get_image(x, y, width, height, X.ZPixmap, 0xffffffff))


def press(connection, symbol):
    code = connection.keysym_to_keycode(symbol)
    xtest.fake_input(connection, X.KeyPress, code)
    xtest.fake_input(connection, X.KeyRelease, code)
    connection.flush()


def references(connection, window):
    size = window.get_geometry()
    top = size.height // 25 + 4  # Exclude the one-row native tab strip and outer border.
    body = (4, top, size.width - 8, size.height - top - 4)

    def has_text():
        raw = capture(window, body)
        return any(len(set(raw[channel::4])) > 8 for channel in range(3))

    wait_for(has_text)
    time.sleep(.8)
    for letter in 'probe ':
        press(connection, ord(letter))
        time.sleep(.05)
    time.sleep(.8)
    before = capture(window, body)
    press(connection, ord('m'))
    time.sleep(.6)
    after = capture(window, body)
    images = [Image.frombytes('RGB', body[2:], raw, 'raw', 'BGRX') for raw in (before, after)]
    bounds = ImageChops.difference(*images).getbbox()
    if not bounds:
        raise RuntimeError('Real Codex did not visibly echo the calibration character')
    left, upper, right, lower = bounds
    left, upper = max(left - 24, 0), max(upper - 24, 0)
    right = min(right + size.width * len(WORD) // 80 + 24, body[2])
    lower = min(lower + 24, body[3])
    area = body[0] + left, body[1] + upper, right - left, lower - upper
    press(connection, XK.XK_BackSpace)
    time.sleep(.6)
    groups = {}
    for index, letter in enumerate(WORD):
        press(connection, ord(letter))
        time.sleep(.4)
        samples = [capture(window, area)]
        # More than one GTK blink period: cursor pixels must not count as text.
        for _ in range(2):
            time.sleep(.45)
            samples.append(capture(window, area))
        groups[index + 1] = samples
    for _ in WORD:
        press(connection, XK.XK_BackSpace)
        time.sleep(.03)
    time.sleep(.6)
    groups[0] = [capture(window, area)]
    for _ in range(2):
        time.sleep(.45)
        groups[0].append(capture(window, area))
    mask = [i for i in range(len(groups[0][0]))
            if any(samples[0][i] != groups[0][0][i] for samples in groups.values())
            and all(all(sample[i] == samples[0][i] for sample in samples[1:]) for samples in groups.values())]
    expected = {key: bytes(samples[0][i] for i in mask) for key, samples in groups.items()}
    if len(mask) < 12 or len(set(expected.values())) != len(groups):
        raise RuntimeError('Could not isolate stable, distinct Codex input glyphs')
    return area, mask, expected


def measure(host, remote, viewer):
    label = 'master' if remote else 'ordinary-ssh'
    directory = os.environ.get('WORMINAL_MASTER_CWD')
    command = (f'cd {shlex.quote(directory)} && ' if directory else '') + 'exec codex'
    daemon = os.environ.get('WORMINAL_MASTER_DAEMON', 'worminald')
    with isolated_display() as env, (CHECKS / f'codex-visible-{label}.log').open('w+b') as log:
        env.update(GDK_BACKEND='x11')
        if remote:
            env.update(PATH=str(ROOT / 'tests/fixtures/preview-master') + os.pathsep + env['PATH'],
                       WORMINAL_TEST_REAL_MASTER=host, WORMINAL_TEST_REMOTE_DAEMON=daemon)
        title = f'Typing probe {label} {os.getpid()}'
        arguments = [str(viewer), '-g', '80x24', '-T', title]
        arguments += ['--master', 'preview-test', '-e', '/bin/sh', '-c', command] if remote else [
            '-e', *SSH[:1], '-tt', '-o', 'BatchMode=yes', host, command]
        owner = subprocess.Popen(arguments, env=env, stdout=log, stderr=log)
        connection = display.Display(env['DISPLAY'])
        window = None
        try:
            window = wait_for(lambda: find_window(connection, title))
            window.set_input_focus(X.RevertToParent, X.CurrentTime)
            connection.sync()
            area, mask, expected = references(connection, window)

            def image():
                raw = capture(window, area)
                return bytes(raw[i] for i in mask)

            wait_for(lambda: image() == expected[0])
            samples = []
            for index, letter in enumerate(WORD):
                started = time.monotonic_ns()
                press(connection, ord(letter))
                wait_for(lambda: image() == expected[index + 1])
                samples.append((time.monotonic_ns() - started) / 1e6)
            summary(f'Real Codex key to visible pixels via {label}', samples)
            return dict(samples_ms=samples, glyph_rectangle=area, stable_bytes=len(mask), draft_submitted=False)
        except Exception:
            if window:
                capture_failure(window)
            raise
        finally:
            stop(owner)
            connection.close()
            if remote:
                scope = env['WORMINAL_SHARED_SOCKET_SCOPE']
                prefix = f'env WORMINAL_SHARED_SOCKET_SCOPE={shlex.quote(scope)} {shlex.quote(daemon)}'
                subprocess.run([*SSH, host, f'{prefix} --stop'], stdout=subprocess.DEVNULL,
                               stderr=subprocess.DEVNULL, timeout=20)


def main():
    CHECKS.mkdir(exist_ok=True)
    viewer = Path(os.environ.get('WORMINAL_CODEX_VIEWER', str(ROOT / 'worminal')))
    results = {}
    for remote in (False, True):
        results['master' if remote else 'ordinary-ssh'] = measure(sys.argv[1], remote, viewer)
        (CHECKS / 'codex-visible-perf.json').write_text(json.dumps(results, indent=2) + '\n')


if __name__ == '__main__':
    main()
