"""Build settings and a real keyboard-to-PTY smoke check."""

from contextlib import nullcontext
import os
import pty
import hashlib
import re
from pathlib import Path
import select
import shutil
import signal
import struct
import subprocess
import time
import unittest

from x11 import isolated_display
from latency import measure_one


ROOT = Path(__file__).resolve().parent.parent


def alignment_display():
    return (nullcontext(os.environ.copy())
            if os.environ.get("WORMINAL_PROOF_PRIVATE_DISPLAY") else isolated_display())


def focus_window(env, window):
    if env.get("WORMINAL_PROOF_PRIVATE_DISPLAY"):
        subprocess.run(["xdotool", "windowactivate", "--sync", window],
                       env=env, check=True)
    subprocess.run(["xdotool", "windowfocus", "--sync", window],
                   env=env, check=True)

    def focused():
        result = subprocess.run(["xdotool", "getwindowfocus"],
                                env=env, capture_output=True, text=True)
        return result.returncode == 0 and result.stdout.strip() == window

    deadline = time.monotonic() + 3
    while time.monotonic() < deadline:
        if focused():
            time.sleep(.05)
            if focused():
                return
        time.sleep(.05)
    raise AssertionError(f"window {window} did not retain X focus")


def window_hash(env, window):
    pixels = subprocess.check_output(["xwd", "-id", window, "-silent"], env=env)
    return hashlib.sha256(pixels).digest()


def tab_strip_hash(env, window):
    image = subprocess.check_output(["xwd", "-id", window, "-silent"], env=env)
    header = struct.unpack(">25I", image[:100])
    offset = header[0] + header[19] * 12
    return hashlib.sha256(image[offset:offset + header[12] * 24]).digest()


def tab_has_underline(env, window):
    image = subprocess.check_output(["xwd", "-id", window, "-silent"], env=env)
    header = struct.unpack(">25I", image[:100])
    offset = header[0] + header[19] * 12
    pixel_size = header[11] // 8
    stride = header[12]
    width = header[4]
    for y in range(4, min(24, header[5])):
        row = image[offset + y * stride:offset + (y + 1) * stride]
        background = row[3 * pixel_size:4 * pixel_size]
        run = 0
        for x in range(10, width - 10):
            pixel = row[x * pixel_size:(x + 1) * pixel_size]
            run = run + 1 if pixel != background else 0
            if run >= 40:
                return True
    return False


def check_placement(env, geometry, expected):
    title = f"Worminal placement {os.getpid()} {'explicit' if geometry else 'default'}"
    manager = subprocess.Popen(
        [str(ROOT / "build/placement_wm")], env=env,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, bufsize=0,
    )
    terminal = None
    try:
        ready, _, _ = select.select([manager.stdout], [], [], 5)
        if not ready or manager.stdout.readline() != b"ready\n":
            raise AssertionError("Private test window manager did not start")
        command = [str(ROOT / "worminal"), "-T", title]
        if geometry:
            command += ["-g", geometry]
        command += ["-e", "/bin/cat"]
        terminal = subprocess.Popen(command, env=env, stdout=subprocess.DEVNULL,
                                    stderr=subprocess.PIPE)

        events = b""
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline and not all(
            event in events for event in (b"mapped\n", b"configured\n")
        ):
            ready, _, _ = select.select([manager.stdout], [], [], 0.1)
            if ready:
                events += os.read(manager.stdout.fileno(), 4096)
            if terminal.poll() is not None:
                raise AssertionError(f"Worminal exited: {terminal.communicate()[1].decode()}")
        if not all(event in events for event in (b"mapped\n", b"configured\n")):
            raise AssertionError(f"Window manager did not see mapping and resize: {events!r}")

        window = subprocess.check_output(
            ["xdotool", "search", "--onlyvisible", "--name", title],
            env=env, text=True,
        ).splitlines()[0]
        lines = subprocess.check_output(
            ["xdotool", "getwindowgeometry", "--shell", window], env=env, text=True,
        ).splitlines()
        position = {key: int(value) for key, value in (line.split("=", 1) for line in lines)}
        if (position["X"], position["Y"]) != expected:
            raise AssertionError(f"Window placed at {position['X']}, {position['Y']}; expected {expected}")
    finally:
        for process in (terminal, manager):
            if process is not None:
                if process.poll() is None:
                    process.terminate()
                try:
                    process.communicate(timeout=2)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.communicate()


def smoke_x11(env):
    result_path = ROOT / "build" / "input"
    result_path.parent.mkdir(exist_ok=True)
    result_path.unlink(missing_ok=True)
    title = f"Worminal smoke {os.getpid()}"
    script = (
        '[ -n "$WINDOWID" ] && [ "$(stty size)" = "24 120" ] || exit 4; '
        r"printf '\033[1mB\033[0m\033[3mI\033[0m\033[1;3mJ\033[0m"
        r"\033[38;5;196mC\033[0m\n'; "
        'IFS= read -r value; [ "$value" = ready ] && printf yes > "$1"'
    )
    env = env.copy()
    env["WORMINAL_TRACE_STARTUP"] = "1"
    process = subprocess.Popen(
        [str(ROOT / "worminal"), "-T", title, "-e", "/bin/sh", "-c", script, "sh", str(result_path)],
        cwd=ROOT,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        deadline = time.monotonic() + 15
        window = None
        while time.monotonic() < deadline:
            search = subprocess.run(
                ["xdotool", "search", "--onlyvisible", "--name", title],
                env=env, capture_output=True, text=True,
            )
            if search.returncode == 0:
                window = search.stdout.splitlines()[0]
                break
            if process.poll() is not None:
                raise AssertionError(f"Worminal exited before opening a window: {process.communicate()[1].decode()}")
            time.sleep(0.05)
        if window is None:
            raise AssertionError("Worminal did not open an X11 window")

        subprocess.run([str(ROOT / "build/icon_probe"), window], env=env, check=True)

        events = []
        trace_buffer = b""
        expected = {("style", 1), ("style", 4), ("style", 5), ("color", 196)}
        while time.monotonic() < deadline and not expected.issubset(events):
            ready, _, _ = select.select([process.stderr], [], [], 0.1)
            if ready:
                trace_buffer += os.read(process.stderr.fileno(), 4096)
                while b"\n" in trace_buffer:
                    line, trace_buffer = trace_buffer.split(b"\n", 1)
                    if line.startswith(b"worminal-startup "):
                        _, event, value = line.decode().split()
                        events.append((event, int(value)))
            if process.poll() is not None:
                break
        if not expected.issubset(events):
            raise AssertionError(f"Styled and indexed colors were not rendered: {events}")
        mapped = events.index(("mapped", 0))
        if any(event in {"style", "color"} for event, _ in events[:mapped]):
            raise AssertionError(f"Palette or font styles loaded before mapping: {events}")

        border_deadline = time.monotonic() + 2
        while time.monotonic() < border_deadline:
            border = subprocess.run([str(ROOT / "build/border_probe"), window],
                                    env=env, capture_output=True, text=True)
            if border.returncode == 0:
                break
            time.sleep(0.05)
        else:
            raise AssertionError(f"Window border was not drawn: {border.stderr}")

        subprocess.run(["xdotool", "windowfocus", "--sync", window], env=env, check=True)
        subprocess.run(["xdotool", "type", "--clearmodifiers", "--delay", "0", "ready"], env=env, check=True)
        subprocess.run(["xdotool", "key", "Return"], env=env, check=True)
        process.communicate(timeout=10)
        if result_path.read_text() != "yes":
            raise AssertionError("The first typed command did not reach the shell")
    finally:
        if process.poll() is None:
            process.kill()
            process.communicate()
        result_path.unlink(missing_ok=True)


class NativeTerminalTest(unittest.TestCase):
    def test_zoom_compact_glyph_stays_in_cursor_cell(self):
        with alignment_display() as env:
            title = f"Worminal glyph alignment {os.getpid()}"
            scoped = {**env, "WORMINAL_SHARED_SOCKET_SCOPE": f"glyph-{os.getpid()}"}
            process = subprocess.Popen(
                [str(ROOT / ".checks/compact-worminal"), "-T", title,
                 "-g", "10x4", "-e", "/bin/sh", "-c",
                 "printf '\\033[?25l\\033[2;3HH'; sleep 30"],
                env=scoped, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
            )
            try:
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline:
                    result = subprocess.run(
                        ["xdotool", "search", "--onlyvisible", "--name", title],
                        env=scoped, capture_output=True, text=True)
                    if result.returncode == 0:
                        window = result.stdout.splitlines()[0]
                        break
                    time.sleep(0.05)
                else:
                    self.fail("Glyph alignment window did not appear")
                focus_window(scoped, window)
                offsets = []
                for step in range(4):
                    if step:
                        subprocess.run(["xdotool", "key", "ctrl+shift+plus"],
                                       env=scoped, check=True)
                    hints = subprocess.check_output(
                        ["xprop", "-id", window, "WM_NORMAL_HINTS"],
                        env=scoped, text=True)
                    match = re.search(r"resize increment: (\d+) by (\d+)", hints)
                    self.assertIsNotNone(match, hints)
                    cw, ch = map(int, match.groups())
                    top = 2 + 2 * ch
                    deadline = time.monotonic() + 3
                    while time.monotonic() < deadline:
                        image = subprocess.check_output(
                            ["xwd", "-id", window, "-silent"], env=scoped)
                        header = struct.unpack(">25I", image[:100])
                        offset = header[0] + header[19] * 12
                        pixel_size, stride = header[11] // 8, header[12]

                        def pixel(x, y):
                            start = offset + y * stride + x * pixel_size
                            return image[start:start + pixel_size]

                        ink = [y for y in range(top - ch, min(header[5] - 2, top + 2 * ch))
                               if any(pixel(x, y) != pixel(2 + 5 * cw, y)
                                      for x in range(2 + 2 * cw, 2 + 3 * cw))]
                        if ink:
                            break
                        time.sleep(0.05)
                    self.assertTrue(ink, f"Glyph missing after zoom step {step}")
                    center = (ink[0] + ink[-1]) / 2
                    offsets.append((step, ch, ink[0], ink[-1], center - (top + ch / 2)))
                self.assertTrue(all(abs(offset[-1]) < offset[1] / 4 for offset in offsets),
                                f"Glyph centers relative to cells: {offsets}")
            finally:
                if process.poll() is None:
                    process.terminate()
                process.communicate(timeout=2)

    def test_zoom_cursor_stays_in_cell(self):
        with alignment_display() as env:
            for style in (2, 6):
                with self.subTest(style=style):
                    scoped = {**env, "WORMINAL_SHARED_SOCKET_SCOPE": f"cursor-{os.getpid()}-{style}"}
                    title = f"Worminal cursor {os.getpid()} {style}"
                    process = subprocess.Popen(
                        [str(ROOT / ".checks/compact-worminal"), "-T", title,
                         "-g", "10x2", "-e", "/bin/sh", "-c",
                         f"printf 'abcdefghij\\033[1;1H\\033[{style} q'; sleep 10"],
                        env=scoped, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                    )
                    try:
                        deadline = time.monotonic() + 5
                        while time.monotonic() < deadline:
                            result = subprocess.run(
                                ["xdotool", "search", "--onlyvisible", "--name", title],
                                env=scoped, capture_output=True, text=True,
                            )
                            if result.returncode == 0:
                                window = result.stdout.splitlines()[0]
                                break
                            time.sleep(0.05)
                        else:
                            self.fail("Cursor test window did not appear")
                        focus_window(scoped, window)
                        subprocess.run(["xdotool", "key", "ctrl+shift+plus"],
                                       env=scoped, check=True)
                        hints = subprocess.check_output(
                            ["xprop", "-id", window, "WM_NORMAL_HINTS"],
                            env=scoped, text=True,
                        )
                        match = re.search(r"resize increment: (\d+) by (\d+)", hints)
                        self.assertIsNotNone(match, hints)
                        cell_width, cell_height = map(int, match.groups())
                        deadline = time.monotonic() + 3
                        while time.monotonic() < deadline:
                            image = subprocess.check_output(
                                ["xwd", "-id", window, "-silent"], env=scoped)
                            header = struct.unpack(">25I", image[:100])
                            offset = header[0] + header[19] * 12
                            pixel_size, stride = header[11] // 8, header[12]
                            top = 2 + cell_height

                            def pixel(x, y):
                                start = offset + y * stride + x * pixel_size
                                return image[start:start + pixel_size]

                            neighbor = 2 + cell_width + 1
                            if (pixel(2, top - 1) == pixel(neighbor, top - 1) and
                                    pixel(2, top) != pixel(neighbor, top) and
                                    pixel(2, top + cell_height - 1) !=
                                    pixel(neighbor, top + cell_height - 1)):
                                break
                            time.sleep(0.05)
                        else:
                            self.fail("Cursor did not occupy its grid cell after zoom")
                        first = 2 + 3 * cell_width + cell_width // 2
                        last = 2 + 4 * cell_width + cell_width // 2
                        for x, action in ((first, "mousedown"), (last, "mouseup")):
                            subprocess.run(["xdotool", "mousemove", "--window", window,
                                            str(x), str(top + cell_height // 2)],
                                           env=scoped, check=True)
                            subprocess.run(["xdotool", action, "1"],
                                           env=scoped, check=True)
                        deadline = time.monotonic() + 3
                        while time.monotonic() < deadline:
                            image = subprocess.check_output(
                                ["xwd", "-id", window, "-silent"], env=scoped)
                            selected_x = 2 + 4 * cell_width - 1
                            plain_x = 2 + 7 * cell_width - 1
                            if (pixel(selected_x, top) != pixel(plain_x, top) and
                                    pixel(selected_x, top + cell_height - 1) !=
                                    pixel(plain_x, top + cell_height - 1) and
                                    pixel(selected_x, top - 1) == pixel(plain_x, top - 1)):
                                break
                            time.sleep(0.05)
                        else:
                            (ROOT / ".checks" / f"selection-{style}.xwd").write_bytes(image)
                            self.fail("Selection did not occupy its grid cell after zoom")
                    finally:
                        if process.poll() is None:
                            process.terminate()
                        process.communicate(timeout=2)

    def test_font_size_shortcuts(self):
        with isolated_display() as env:
            title = f"Worminal zoom {os.getpid()}"
            process = subprocess.Popen(
                [str(ROOT / "worminal"), "-T", title, "-e", "/bin/cat"],
                env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
            )
            try:
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline:
                    result = subprocess.run(
                        ["xdotool", "search", "--onlyvisible", "--name", title],
                        env=env, capture_output=True, text=True,
                    )
                    if result.returncode == 0:
                        window = result.stdout.splitlines()[0]
                        break
                    time.sleep(0.05)
                else:
                    self.fail("Zoom test window did not appear")

                focus_window(env, window)

                def increment():
                    hints = subprocess.check_output(
                        ["xprop", "-id", window, "WM_NORMAL_HINTS"], env=env,
                        text=True,
                    )
                    match = re.search(r"resize increment: (\d+) by (\d+)", hints)
                    self.assertIsNotNone(match, hints)
                    return tuple(map(int, match.groups()))

                def wait_for_change(previous):
                    deadline = time.monotonic() + 3
                    while time.monotonic() < deadline:
                        current = increment()
                        if current != previous:
                            return current
                        time.sleep(0.02)
                    self.fail(f"Font size did not change from {previous}")

                original = increment()
                subprocess.run(["xdotool", "key", "ctrl+shift+plus"], env=env, check=True)
                larger = wait_for_change(original)
                self.assertGreater(larger[1], original[1])
                subprocess.run(["xdotool", "key", "ctrl+minus"], env=env, check=True)
                self.assertEqual(wait_for_change(larger), original)
                subprocess.run(["xdotool", "key", "ctrl+equal"], env=env, check=True)
                self.assertEqual(wait_for_change(original), larger)
            finally:
                if process.poll() is None:
                    process.terminate()
                process.communicate(timeout=5)

    def test_zoom_reflow_matches_fresh_render(self):
        text = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ"
        script = (
            "stty -echo; IFS= read -r trigger; "
            f"printf '\\033[2 q\\033[7m%s\\033[0m' '{text}'; "
            ': > "$1"; exec sleep 30'
        )
        with isolated_display() as env:
            windows = []
            processes = []
            markers = []
            try:
                for order in ("after", "before"):
                    scoped = {**env, "WORMINAL_SHARED_SOCKET_SCOPE":
                              f"reflow-{os.getpid()}-{order}"}
                    marker = ROOT / ".checks" / f"reflow-{os.getpid()}-{order}"
                    marker.unlink(missing_ok=True)
                    markers.append(marker)
                    title = f"Worminal reflow {os.getpid()} {order}"
                    process = subprocess.Popen(
                        [str(ROOT / ".checks/compact-worminal"), "-T", title,
                         "-g", "20x4", "-e", "/bin/sh", "-c", script, "sh", str(marker)],
                        env=scoped, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                    )
                    processes.append(process)
                    deadline = time.monotonic() + 5
                    while time.monotonic() < deadline:
                        result = subprocess.run(
                            ["xdotool", "search", "--onlyvisible", "--name", title],
                            env=scoped, capture_output=True, text=True,
                        )
                        if result.returncode == 0:
                            window = result.stdout.splitlines()[0]
                            break
                        time.sleep(0.05)
                    else:
                        self.fail(f"Reflow {order} window did not appear")
                    focus_window(scoped, window)
                    if order == "before":
                        subprocess.run(["xdotool", "key", "ctrl+shift+plus"],
                                       env=scoped, check=True)
                    subprocess.run(["xdotool", "key", "Return"], env=scoped, check=True)
                    deadline = time.monotonic() + 5
                    while not marker.exists() and time.monotonic() < deadline:
                        time.sleep(0.02)
                    self.assertTrue(marker.exists(), f"Reflow {order} output was not written")
                    if order == "after":
                        subprocess.run(["xdotool", "key", "ctrl+shift+plus"],
                                       env=scoped, check=True)
                    windows.append((scoped, window))

                hints = subprocess.check_output(
                    ["xprop", "-id", windows[0][1], "WM_NORMAL_HINTS"],
                    env=windows[0][0], text=True)
                match = re.search(r"resize increment: (\d+) by (\d+)", hints)
                self.assertIsNotNone(match, hints)
                _, cell_height = map(int, match.groups())

                def pixels(scoped, window):
                    image = subprocess.check_output(
                        ["xwd", "-id", window, "-silent"], env=scoped)
                    header = struct.unpack(">25I", image[:100])
                    offset = header[0] + header[19] * 12
                    data = image[offset:offset + header[5] * header[12]]
                    return header[4:6], data, image, header[11] // 8, header[12]

                def capture_pair():
                    pair = []
                    for scoped, window in windows:
                        subprocess.run(["xdotool", "windowraise", window],
                                       env=scoped, check=True)
                        focus_window(scoped, window)
                        pair.append(pixels(scoped, window))
                    return pair

                def wait_for_matching_render(previous=None):
                    deadline = time.monotonic() + 3
                    while time.monotonic() < deadline:
                        pair = capture_pair()
                        (width, _), data, _, pixel_size, stride = pair[0]
                        # Reverse video fills the first cell; the far corner is
                        # blank. This prevents two unfinished frames passing.
                        def pixel(x, y):
                            start = y * stride + x * pixel_size
                            return data[start:start + pixel_size]
                        rendered = (pixel(3, 3 + cell_height) !=
                                    pixel(width - 3, 3 + 3 * cell_height))
                        matching = pair[0][:2] == pair[1][:2]
                        changed = previous is None or pair[0][:2] != previous[0][:2]
                        if rendered and matching and changed:
                            return pair
                        time.sleep(0.05)
                    for index, capture in enumerate(pair):
                        (ROOT / ".checks" / f"reflow-capture-{index}.xwd").write_bytes(capture[2])
                    self.fail("Zoom render did not settle to matching frames; "
                              "XWD captures are in .checks/reflow-capture-*.xwd")

                plain = wait_for_matching_render()
                for scoped, window in windows:
                    subprocess.run(["xdotool", "windowraise", window],
                                   env=scoped, check=True)
                    focus_window(scoped, window)
                    hints = subprocess.check_output(
                        ["xprop", "-id", window, "WM_NORMAL_HINTS"],
                        env=scoped, text=True,
                    )
                    match = re.search(r"resize increment: (\d+) by (\d+)", hints)
                    self.assertIsNotNone(match, hints)
                    cw, ch = map(int, match.groups())
                    for x, y, action in ((2 + cw, 2 + ch + 1, "mousedown"),
                                         (2 + 4 * cw, 2 + 3 * ch + 1, "mouseup")):
                        subprocess.run(["xdotool", "mousemove", "--window", window,
                                        str(x), str(y)], env=scoped, check=True)
                        subprocess.run(["xdotool", action, "1"], env=scoped, check=True)
                wait_for_matching_render(plain)
            finally:
                for process in processes:
                    if process.poll() is None:
                        process.kill()
                    process.communicate(timeout=5)
                for marker in markers:
                    marker.unlink(missing_ok=True)

    def test_new_executable_joins_existing_workspace(self):
        with isolated_display() as env:
            copy = ROOT / "build" / f"worminal-copy-{os.getpid()}"
            shutil.copy2(ROOT / "worminal", copy)
            owners = []
            try:
                for binary, title in ((ROOT / "worminal", "Earlier build"),
                                      (copy, "Later build")):
                    owner = subprocess.Popen(
                        [str(binary), "-T", title, "-e", "/bin/cat"], env=env,
                        stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
                    owners.append(owner)
                    deadline = time.monotonic() + 5
                    while time.monotonic() < deadline:
                        result = subprocess.run(
                            ["xdotool", "search", "--onlyvisible", "--pid", str(owner.pid)],
                            env=env, capture_output=True, text=True)
                        if result.returncode == 0:
                            break
                        if owner.poll() is not None:
                            self.fail("client exited before opening its window")
                        time.sleep(.05)
                    else:
                        self.fail("new executable did not create its own window")
                self.assertTrue(all(owner.poll() is None for owner in owners))
                second = subprocess.check_output(
                    ["xdotool", "search", "--onlyvisible", "--pid", str(owners[1].pid)],
                    env=env, text=True).splitlines()[0]
                focus_window(env, second)
                subprocess.run(["xdotool", "key", "ctrl+1"], env=env, check=True)
                self.assertEqual(subprocess.check_output(
                    ["xdotool", "getwindowname", second], env=env, text=True).strip(),
                    "Earlier build")
            finally:
                for owner in owners:
                    if owner.poll() is None:
                        owner.terminate()
                    owner.communicate(timeout=2)
                copy.unlink(missing_ok=True)

    def test_tab_label_tracks_child_directory(self):
        with isolated_display() as env:
            env = {**env, "SHELL": "/bin/sh"}
            title = f"Worminal directory {os.getpid()}"
            marker = ROOT / "build" / f"directory-ready-{os.getpid()}"
            new_tab_pwd = ROOT / "build" / f"new-tab-pwd-{os.getpid()}"
            marker.unlink(missing_ok=True)
            new_tab_pwd.unlink(missing_ok=True)
            process = subprocess.Popen(
                [str(ROOT / "worminal"), "-T", title, "-e", "/bin/sh", "-c",
                 'while IFS= read -r command; do eval "$command"; done'],
                cwd=ROOT, env=env, stdout=subprocess.DEVNULL,
                stderr=subprocess.PIPE)

            def wait_for(predicate, message):
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline:
                    result = predicate()
                    if result:
                        return result
                    time.sleep(.05)
                self.fail(message)

            try:
                window = wait_for(
                    lambda: (subprocess.run(
                        ["xdotool", "search", "--onlyvisible", "--name", title],
                        env=env, capture_output=True, text=True).stdout.splitlines() or [None])[0],
                    "directory test window did not open")
                focus_window(env, window)

                def send(command):
                    marker.unlink(missing_ok=True)
                    subprocess.run(["xdotool", "type", "--clearmodifiers", "--delay", "0",
                                    command], env=env, check=True)
                    subprocess.run(["xdotool", "key", "Return"], env=env, check=True)
                    wait_for(marker.exists, "shell did not finish directory command")
                    previous = None
                    stable = 0
                    deadline = time.monotonic() + 3
                    while stable < 3 and time.monotonic() < deadline:
                        current = tab_strip_hash(env, window)
                        stable = stable + 1 if current == previous else 0
                        previous = current
                        time.sleep(.05)
                    self.assertEqual(stable, 3, "tab strip did not settle")
                    return previous

                clear = f"printf '\\033[2J\\033[H\\033[?25l'; : > {marker}"
                before = send(clear)
                self.assertTrue(tab_has_underline(env, window),
                                "tab label has no visible underline")
                after = send(f"cd {ROOT / 'build'}; {clear}")
                self.assertNotEqual(before, after,
                                    "tab label did not follow the shell directory")
                subprocess.run([str(ROOT / "build/border_probe"), window],
                               env=env, check=True)
                self.assertEqual(subprocess.check_output(
                    ["xdotool", "getwindowname", window], env=env,
                    text=True).strip(), title)
                subprocess.run(["xdotool", "key", "ctrl+t"], env=env, check=True)
                subprocess.run(["xdotool", "type", "--clearmodifiers", "--delay", "0",
                                f"pwd > {new_tab_pwd}"], env=env, check=True)
                subprocess.run(["xdotool", "key", "Return"], env=env, check=True)
                wait_for(new_tab_pwd.exists, "new tab did not run pwd")
                self.assertEqual(new_tab_pwd.read_text().strip(), str(ROOT / "build"),
                                 "new tab did not inherit the selected tab's directory")
            finally:
                if process.poll() is None:
                    process.terminate()
                process.communicate(timeout=2)
                marker.unlink(missing_ok=True)
                new_tab_pwd.unlink(missing_ok=True)

    def test_forwarded_line_keeps_owner_stdin(self):
        with isolated_display() as env:
            master, slave = pty.openpty()
            owner = subprocess.Popen(
                [str(ROOT / "worminal"), "-e", "/bin/cat"],
                env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            second = None

            def windows():
                result = subprocess.run(
                    ["xdotool", "search", "--onlyvisible", "--class", "Worminal"],
                    env=env, capture_output=True, text=True)
                return result.stdout.splitlines() if result.returncode == 0 else []

            try:
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline and len(windows()) < 1:
                    time.sleep(.05)
                self.assertEqual(len(windows()), 1)
                stdin_before = os.readlink(f"/proc/{owner.pid}/fd/0")
                second = subprocess.Popen([str(ROOT / "worminal"), "-l", os.ttyname(slave)],
                                          env=env, stdout=subprocess.DEVNULL,
                                          stderr=subprocess.PIPE)
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline and len(windows()) < 2:
                    time.sleep(.05)
                self.assertEqual(len(windows()), 2)
                self.assertEqual(os.readlink(f"/proc/{owner.pid}/fd/0"), stdin_before)
                for window in windows():
                    subprocess.run(["xdotool", "windowclose", window], env=env,
                                   check=True)
                owner.wait(timeout=3)
                self.assertEqual(owner.returncode, 0)
                second.wait(timeout=3)
                self.assertEqual(second.returncode, 0)
            finally:
                for process in (owner, second):
                    if process is not None:
                        if process.poll() is None:
                            process.terminate()
                        process.wait(timeout=2)
                        process.stderr.close()
                os.close(master)
                os.close(slave)

    def test_hidden_tab_drains_and_launcher_state_is_forwarded(self):
        with isolated_display() as env:
            a_title = f"Worminal A {os.getpid()}"
            b_title = f"Worminal B {os.getpid()}"
            prefix = ROOT / "build" / f"hidden-{os.getpid()}"
            info, gate, done = (Path(f"{prefix}-{part}") for part in
                                ("info", "gate", "done"))
            for path in (info, gate, done):
                path.unlink(missing_ok=True)
            owner = subprocess.Popen(
                [str(ROOT / "worminal"), "-T", a_title, "-e", "/bin/cat"],
                env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            second_process = None

            def wait_for(predicate, message):
                deadline = time.monotonic() + 6
                while time.monotonic() < deadline:
                    value = predicate()
                    if value:
                        return value
                    time.sleep(.05)
                self.fail(message)

            def windows():
                result = subprocess.run(
                    ["xdotool", "search", "--onlyvisible", "--class", "Worminal"],
                    env=env, capture_output=True, text=True)
                return result.stdout.splitlines() if result.returncode == 0 else []

            def title(window):
                return subprocess.check_output(
                    ["xdotool", "getwindowname", window], env=env, text=True).strip()

            try:
                first = wait_for(lambda: windows()[0] if windows() else None,
                                 "first window did not open")
                second_env = {**env, "WORMINAL_TEST_VALUE": "forwarded"}
                script = ('printf "\\033]0;temporary\\007\\033c"; '
                          'printf "%s|%s|%s|%s" "$WORMINAL_TEST_VALUE" '
                          '"$(pwd)" "$(stty size)" "$WINDOWID" > "$1"; '
                          'while [ ! -e "$2" ]; do sleep .02; done; '
                          'yes hidden | head -c 1048576; printf done > "$3"; '
                          'while :; do sleep 1; done')
                second_process = subprocess.Popen(
                    [str(ROOT / "worminal"), "-T", b_title, "-g", "40x10",
                     "-e", "/bin/sh", "-c", script, "sh",
                     str(info), str(gate), str(done)],
                    cwd=ROOT / "build", env=second_env,
                    stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
                second = wait_for(lambda: next((w for w in windows() if w != first), None),
                                  "second window did not open")
                details = wait_for(lambda: info.read_text() if info.exists() else None,
                                   "forwarded shell did not start").split("|")
                self.assertEqual(details, ["forwarded", str(ROOT / "build"),
                                           "10 40", second])
                wait_for(lambda: title(second) == b_title,
                         "terminal reset lost its launcher's title")
                focus_window(env, second)
                subprocess.run(["xdotool", "key", "ctrl+1"], env=env, check=True)
                wait_for(lambda: title(second) == a_title, "Ctrl+1 did not select tab A")
                frozen = window_hash(env, second)
                gate.touch()
                wait_for(lambda: done.exists(), "hidden tab stopped draining its PTY")
                self.assertEqual(window_hash(env, second), frozen,
                                 "hidden tab repainted the terminal area")
                subprocess.run(["xdotool", "key", "ctrl+Tab"], env=env, check=True)
                wait_for(lambda: title(second) == b_title, "Ctrl+Tab did not select tab B")
                self.assertNotEqual(window_hash(env, second), frozen)
                subprocess.run(["xdotool", "key", "ctrl+shift+Tab"], env=env,
                               check=True)
                wait_for(lambda: title(second) == a_title,
                         "Ctrl+Shift+Tab did not select tab A")
                subprocess.run(["xdotool", "mousemove", "--window", second,
                                "10", "10", "click", "1"], env=env, check=True)
                self.assertEqual(title(second), a_title)
                subprocess.run(["xdotool", "key", "ctrl+t"], env=env, check=True)
                wait_for(lambda: title(second) not in (a_title, b_title),
                         "Ctrl+T did not create a tab")
                subprocess.run(["xdotool", "key", "ctrl+w"], env=env, check=True)
                wait_for(lambda: title(second) == b_title,
                         "Ctrl+W did not return to the previous tab")
                subprocess.run(["xdotool", "key", "ctrl+1"], env=env, check=True)
                wait_for(lambda: title(second) == a_title, "Ctrl+1 did not select tab A")
                subprocess.run(["xdotool", "key", "ctrl+9"], env=env, check=True)
                wait_for(lambda: title(second) == b_title, "Ctrl+9 did not select last tab")
                subprocess.run(["xdotool", "key", "alt+1"], env=env, check=True)
                wait_for(lambda: title(second) == a_title, "Alt+1 did not select tab A")
                for key in ("alt+9", "alt+0"):
                    subprocess.run(["xdotool", "key", key], env=env, check=True)
                    time.sleep(.05)
                    self.assertEqual(title(second), a_title,
                                     f"{key} selected an absent tab slot")
                subprocess.run(["xdotool", "key", "alt+2"], env=env, check=True)
                wait_for(lambda: title(second) == b_title, "Alt+2 did not select tab B")
                focus_window(env, second)
                subprocess.run(["xdotool", "key", "ctrl+n"], env=env, check=True)
                third = wait_for(lambda: next((w for w in windows()
                                               if w not in (first, second)), None),
                                 "Ctrl+N did not open a window")
                self.assertNotIn(title(third), (a_title, b_title))
                self.assertEqual(title(second), b_title,
                                 "new window changed another window's selected tab")
                subprocess.run(["xdotool", "windowclose", third], env=env, check=True)
                wait_for(lambda: len(windows()) == 2, "third window did not close")
                focus_window(env, second)
                subprocess.run(["xdotool", "key", "ctrl+9"], env=env, check=True)
                wait_for(lambda: title(second) not in (a_title, b_title),
                         "tab died when its only window closed")
                subprocess.run(["xdotool", "key", "ctrl+w"], env=env, check=True)
                wait_for(lambda: title(second) == b_title,
                         "hidden tab did not close globally")
                subprocess.run(["xdotool", "windowclose", first], env=env, check=True)
                wait_for(lambda: len(windows()) == 1, "first window did not close")
                self.assertEqual(owner.wait(timeout=3), 0)
                self.assertIsNone(second_process.poll(), "remaining window exited")
                subprocess.run(["xdotool", "windowclose", second], env=env, check=True)
                owner.wait(timeout=3)
                self.assertEqual(owner.returncode, 0)
                second_process.wait(timeout=3)
                self.assertEqual(second_process.returncode, 0)
            finally:
                for process in (owner, second_process):
                    if process is not None:
                        if process.poll() is None:
                            process.terminate()
                        process.wait(timeout=2)
                        process.stderr.close()
                for path in (info, gate, done):
                    path.unlink(missing_ok=True)

    def test_independent_tabs_share_one_owner(self):
        display = (nullcontext(os.environ.copy())
                   if os.environ.get("WORMINAL_PROOF_PRIVATE_DISPLAY") else isolated_display())
        with display as env:
            title = f"Worminal tabs {os.getpid()}"
            first_tty = ROOT / "build" / "tab_first_tty"
            first_input = ROOT / "build" / "tab_first_input"
            second_tty = ROOT / "build" / "tab_second_tty"
            second_input = ROOT / "build" / "tab_second_input"
            second_window = ROOT / "build" / "tab_second_window"
            paths = (first_tty, first_input, second_tty, second_input, second_window)
            for path in paths:
                path.unlink(missing_ok=True)
            script = ('tty > "$1"; while IFS= read -r line; do '
                      'printf "%s\\n" "$line" >> "$2"; '
                      'printf "[%s]\\n" "$line"; done')
            owner = subprocess.Popen(
                [str(ROOT / "worminal"), "-T", title, "-e", "/bin/sh",
                 "-c", script, "sh", str(first_tty), str(first_input)],
                env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
            )
            second_process = None

            def windows(count):
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline:
                    found = subprocess.run(
                        ["xdotool", "search", "--onlyvisible", "--class", "Worminal"],
                        env=env, capture_output=True, text=True)
                    ids = found.stdout.splitlines() if found.returncode == 0 else []
                    if len(ids) == count:
                        return ids
                    if owner.poll() is not None:
                        self.fail(f"owner exited: {owner.communicate()[1].decode()}")
                    time.sleep(.05)
                self.fail(f"expected {count} tab windows; saw {ids}")

            def focus(window):
                focus_window(env, window)

            def type_line(line):
                subprocess.run(["xdotool", "type", "--clearmodifiers", "--delay", "10", line],
                               env=env, check=True)
                subprocess.run(["xdotool", "key", "Return"], env=env, check=True)

            def wait_file(path, expected=None):
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline:
                    if path.exists():
                        content = path.read_text()
                        if content and (expected is None or content == expected):
                            return content
                    time.sleep(.05)
                self.fail(f"tab did not write {expected!r} to {path.name}")

            def image_hash(window):
                return window_hash(env, window)

            try:
                first = windows(1)[0]
                wait_file(first_tty)
                second_process = subprocess.Popen([str(ROOT / "worminal")], env=env,
                                                  stdout=subprocess.DEVNULL,
                                                  stderr=subprocess.PIPE)
                second = next(window for window in windows(2) if window != first)
                if os.environ.get("WORMINAL_PROOF_PRIVATE_DISPLAY"):
                    for window, x in ((first, "20"), (second, "380")):
                        subprocess.run(["xdotool", "windowsize", window, "300", "250"],
                                       env=env, check=True)
                        subprocess.run(["xdotool", "windowmove", window, x, "20"],
                                       env=env, check=True)
                focus(second)
                type_line(f"tty > {second_tty}; printf second > {second_input}; "
                          f"printf %s \"$WINDOWID\" > {second_window}")
                self.assertEqual(wait_file(second_input), "second")
                self.assertNotEqual(first_tty.read_text(), wait_file(second_tty),
                                    "tabs used the same PTY")
                self.assertEqual(wait_file(second_window), second,
                                 "second shell inherited the wrong window")
                focus(first)
                second_before = image_hash(second)
                type_line("alpha")
                self.assertEqual(wait_file(first_input, "alpha\n"), "alpha\n")
                self.assertEqual(image_hash(second), second_before,
                                 "first tab output changed the second tab")
                focus(second)
                type_line(f"printf more >> {second_input}")
                self.assertEqual(wait_file(second_input, "secondmore"), "secondmore")
                type_line("exit")
                time.sleep(.2)
                self.assertIsNone(owner.poll(), "second shell exit ended the owner")
                subprocess.run(["xdotool", "windowclose", second], env=env, check=True)
                self.assertEqual(windows(1), [first])
                self.assertIsNone(owner.poll(), "closing one tab ended the owner")
                focus(first)
                type_line("after")
                self.assertEqual(wait_file(first_input, "alpha\nafter\n"),
                                 "alpha\nafter\n")
                subprocess.run(["xdotool", "windowclose", first], env=env, check=True)
                owner.wait(timeout=3)
                self.assertEqual(owner.returncode, 0)
                second_process.wait(timeout=3)
                self.assertEqual(second_process.returncode, 0)
            finally:
                for process in (owner, second_process):
                    if process is not None:
                        if process.poll() is None:
                            process.terminate()
                        process.wait(timeout=2)
                        process.stderr.close()
                owner.stderr.close()
                for path in paths:
                    path.unlink(missing_ok=True)
                subprocess.run([str(ROOT / "worminald"), "--stop"], env=env, check=True)
    def test_close_during_early_map(self):
        with isolated_display() as env:
            manager = subprocess.Popen(
                [str(ROOT / "build/placement_wm")],
                env={**env, "WORMINAL_CLOSE_ON_MAP": "1"},
                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            )
            terminal = None
            try:
                ready, _, _ = select.select([manager.stdout], [], [], 5)
                self.assertTrue(ready and manager.stdout.readline() == b"ready\n")
                terminal = subprocess.Popen(
                    [str(ROOT / "worminal"), "-e", "/bin/cat"],
                    env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                )
                ready, _, _ = select.select([manager.stdout], [], [], 5)
                self.assertTrue(ready and manager.stdout.readline() == b"closed\n")
                terminal.wait(timeout=3)
                self.assertEqual(terminal.returncode, 0, terminal.stderr.read().decode())
            finally:
                for process in (terminal, manager):
                    if process is not None:
                        if process.poll() is None:
                            process.terminate()
                        process.wait(timeout=2)
                        for stream in (process.stdout, process.stderr):
                            if stream is not None:
                                stream.close()

    def test_shared_owner_election_and_restart(self):
        with isolated_display() as env:
            title = f"Worminal race {os.getpid()}"
            child_pids = ROOT / "build" / f"shared-race-pids-{os.getpid()}"
            child_pids.unlink(missing_ok=True)
            command = [str(ROOT / "worminal"), "-T", title,
                       "-e", "/bin/sh", "-c",
                       'trap "" HUP; printf "%s\n" "$$" >> "$1"; exec sleep 30',
                       "sh", str(child_pids)]
            launchers = [subprocess.Popen(command, env=env, stdout=subprocess.DEVNULL,
                                          stderr=subprocess.PIPE) for _ in range(2)]
            try:
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline:
                    result = subprocess.run(["xdotool", "search", "--onlyvisible", "--name", title],
                                            env=env, capture_output=True, text=True)
                    windows = result.stdout.splitlines() if result.returncode == 0 else []
                    if len(windows) == 2 and child_pids.exists() and len(child_pids.read_text().splitlines()) == 2:
                        break
                    time.sleep(.05)
                else:
                    self.fail(f"Concurrent clients did not create two tabs: {windows}")
                self.assertTrue(all(process.poll() is None for process in launchers))
                for window in windows:
                    subprocess.run(["xdotool", "windowclose", window], env=env, check=True)
                for process in launchers:
                    self.assertEqual(process.wait(timeout=3), 0)
                import socket
                connection = socket.socket(socket.AF_UNIX)
                try:
                    scope = env["WORMINAL_SHARED_SOCKET_SCOPE"]
                    connection.connect(f"\0worminal-workspace-{os.getuid()}-{scope}")
                finally:
                    connection.close()
                restarted = subprocess.Popen(command, env=env, stdout=subprocess.DEVNULL,
                                             stderr=subprocess.PIPE)
                launchers.append(restarted)
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline:
                    result = subprocess.run(["xdotool", "search", "--onlyvisible", "--name", title],
                                            env=env, capture_output=True, text=True)
                    if result.returncode == 0 and len(child_pids.read_text().splitlines()) == 3:
                        break
                    time.sleep(.05)
                else:
                    self.fail("Workspace did not accept a new tab after all windows closed")
                subprocess.run(["xdotool", "windowclose", result.stdout.splitlines()[0]],
                               env=env, check=True)
                self.assertEqual(restarted.wait(timeout=3), 0)
            finally:
                for process in launchers:
                    if process.poll() is None:
                        process.terminate()
                    process.wait(timeout=2)
                    process.stderr.close()
                if child_pids.exists():
                    for line in child_pids.read_text().splitlines():
                        try:
                            os.kill(int(line), signal.SIGTERM)
                        except ProcessLookupError:
                            pass
                    child_pids.unlink()

    def test_shared_view_end_to_end(self):
        display = (nullcontext(os.environ.copy())
                   if os.environ.get("WORMINAL_PROOF_PRIVATE_DISPLAY") else isolated_display())
        with display as env:
            title = f"Worminal shared {os.getpid()}"
            received = ROOT / "build" / "shared_input"
            sizes = ROOT / "build" / "shared_sizes"
            drained = ROOT / "build" / "shared_drained"
            received.unlink(missing_ok=True)
            sizes.unlink(missing_ok=True)
            drained.unlink(missing_ok=True)
            script = (
                "printf '\033[?25l'; "
                'while IFS= read -r line; do '
                'case "$line" in '
                'size) stty size >> "$2";; '
                "alt) printf '\\033[?1049h[ALT]\\n';; "
                "leave) printf '\\033[?1049l';; "
                "bulk) printf started > \"$3\"; "
                "yes '0123456789abcdefghijklmnopqrstuvwxyz' | head -c 1048576; "
                'printf done > "$3";; esac; '
                'printf "[%s]\\n" "$line"; '
                'printf "%s\\n" "$line" >> "$1"; done'
            )
            owner = subprocess.Popen(
                [str(ROOT / "worminal"), "-T", title, "-e", "/bin/sh",
                 "-c", script, "sh", str(received), str(sizes), str(drained)],
                env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
            )
            second_process = None

            def windows(count):
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline:
                    found = subprocess.run(
                        ["xdotool", "search", "--onlyvisible", "--class", "Worminal"],
                        env=env, capture_output=True, text=True,
                    )
                    ids = found.stdout.splitlines() if found.returncode == 0 else []
                    if len(ids) == count:
                        return ids
                    if owner.poll() is not None:
                        self.fail(f"Shared owner exited: {owner.communicate()[1].decode()}")
                    time.sleep(.05)
                self.fail(f"Expected {count} shared windows; saw {ids}")

            def focus(window):
                focus_window(env, window)

            def typed(value):
                subprocess.run(["xdotool", "type", "--clearmodifiers", "--delay", "10", value],
                               env=env, check=True)
                subprocess.run(["xdotool", "key", "Return"], env=env, check=True)
                deadline = time.monotonic() + 3
                while time.monotonic() < deadline:
                    if received.exists() and value in received.read_text().splitlines():
                        return
                    time.sleep(.05)
                self.fail(f"Input {value!r} did not reach the shared shell: "
                          f"received={received.read_text() if received.exists() else None!r}, "
                          f"drained={drained.read_text() if drained.exists() else None!r}, "
                          f"clients={[owner.poll(), second_process.poll()]}")

            def image_hash(window):
                return window_hash(env, window)

            def geometry(window):
                lines = subprocess.check_output(
                    ["xdotool", "getwindowgeometry", "--shell", window], env=env, text=True)
                return {key: int(value) for key, value in
                        (line.split("=", 1) for line in lines.splitlines())}

            def size_count(count):
                deadline = time.monotonic() + 3
                while time.monotonic() < deadline:
                    lines = sizes.read_text().splitlines() if sizes.exists() else []
                    if len(lines) >= count:
                        return lines
                    time.sleep(.05)
                self.fail(f"PTY did not report {count} sizes: {lines}")

            try:
                first = windows(1)[0]
                second_process = subprocess.Popen([str(ROOT / "worminal")], env=env,
                                                  stdout=subprocess.DEVNULL,
                                                  stderr=subprocess.PIPE)
                deadline = time.monotonic() + 5
                second = None
                while time.monotonic() < deadline and second is None:
                    found = subprocess.run(
                        ["xdotool", "search", "--onlyvisible", "--pid", str(second_process.pid)],
                        env=env, capture_output=True, text=True)
                    second = next((window for window in found.stdout.splitlines()
                                   if window != first), None)
                    if second is None:
                        time.sleep(.05)
                self.assertIsNotNone(second, "second shared window did not map")
                deadline = time.monotonic() + 3
                while time.monotonic() < deadline:
                    focus(second)
                    subprocess.run(["xdotool", "key", "ctrl+1"], env=env, check=True)
                    name = subprocess.check_output(
                        ["xdotool", "getwindowname", second], env=env, text=True).strip()
                    if name == title:
                        break
                    time.sleep(.05)
                else:
                    first_name = subprocess.check_output(
                        ["xdotool", "getwindowname", first], env=env, text=True).strip()
                    self.fail(f"second view never selected first tab: first={first_name!r}, second={name!r}")
                self.assertEqual(set(windows(2)), {first, second})
                if os.environ.get("WORMINAL_PROOF_PRIVATE_DISPLAY"):
                    for window, x in ((first, "20"), (second, "380")):
                        subprocess.run(["xdotool", "windowsize", window, "300", "250"],
                                       env=env, check=True)
                        subprocess.run(["xdotool", "windowmove", window, x, "20"],
                                       env=env, check=True)
                focus(second)
                focus(first)
                before = image_hash(second)
                typed("alpha")
                self.assertEqual(image_hash(second), before,
                                 "inactive view received a PTY repaint")
                focus(second)
                self.assertNotEqual(image_hash(second), before,
                                    "reactivated view did not catch up")
                typed("beta")
                subprocess.run(["xdotool", "windowsize", second, "600", "300"],
                               env=env, check=True)
                deadline = time.monotonic() + 3
                while time.monotonic() < deadline and geometry(second)["WIDTH"] < 500:
                    time.sleep(.05)
                self.assertGreaterEqual(geometry(second)["WIDTH"], 500,
                                        "window manager did not apply the test resize")
                focus(second)
                second_geometry = geometry(second)
                typed("size")
                second_size = size_count(1)[-1]
                focus(first)
                first_geometry = geometry(first)
                typed("size")
                first_size = size_count(2)[-1]
                self.assertNotEqual(first_size, second_size,
                                    "live-view handoff did not resize the PTY; "
                                    f"first={first_geometry!r} second={second_geometry!r}")
                focus(second)
                typed("size")
                self.assertEqual(size_count(3)[-1], second_size)
                typed("alt")
                focus(first)
                focus(second)
                typed("leave")
                focus(first)
                frozen = image_hash(second)
                typed("bulk")
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline and not drained.exists():
                    time.sleep(.05)
                self.assertEqual(drained.read_text() if drained.exists() else None, "done")
                self.assertEqual(image_hash(second), frozen,
                                 "inactive view repainted during a large PTY write")
                focus(second)
                self.assertNotEqual(image_hash(second), frozen,
                                    "inactive view did not catch up after bulk output")
                subprocess.run(["xdotool", "windowclose", first], env=env, check=True)
                self.assertEqual(windows(1), [second], "first view did not close")
                owner.wait(timeout=3)
                self.assertEqual(owner.returncode, 0)
                focus(second)
                typed("gamma")
                self.assertEqual(received.read_text().splitlines(),
                                 ["alpha", "beta", "size", "size", "size",
                                  "alt", "leave", "bulk", "gamma"])
                subprocess.run(["xdotool", "windowclose", second], env=env, check=True)
                owner.wait(timeout=3)
                self.assertEqual(owner.returncode, 0)
                second_process.wait(timeout=3)
                self.assertEqual(second_process.returncode, 0)
            finally:
                for process in (owner, second_process):
                    if process is not None:
                        if process.poll() is None:
                            process.terminate()
                        process.wait(timeout=2)
                        process.stderr.close()
                received.unlink(missing_ok=True)
                sizes.unlink(missing_ok=True)
                drained.unlink(missing_ok=True)
                subprocess.run([str(ROOT / "worminald"), "--stop"], env=env, check=True)
    def test_view_contexts_draw_independently(self):
        with isolated_display() as env:
            subprocess.run([str(ROOT / "build/view_state_test")], env=env, check=True)

    def test_unmapped_window_keeps_draining_pty(self):
        with isolated_display() as env:
            gate = ROOT / "build" / "drain_gate"
            done = ROOT / "build" / "drain_done"
            gate.unlink(missing_ok=True)
            done.unlink(missing_ok=True)
            title = f"Worminal drain {os.getpid()}"
            script = (
                'while [ ! -e "$1" ]; do sleep .02; done; '
                "yes '0123456789abcdefghijklmnopqrstuvwxyz' | head -c 1048576; "
                'printf done > "$2"; sleep 5'
            )
            process = subprocess.Popen(
                [str(ROOT / "worminal"), "-T", title, "-e", "/bin/sh", "-c",
                 script, "sh", str(gate), str(done)],
                env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
            )
            try:
                deadline = time.monotonic() + 5
                window = None
                while time.monotonic() < deadline:
                    search = subprocess.run(
                        ["xdotool", "search", "--onlyvisible", "--name", title],
                        env=env, capture_output=True, text=True,
                    )
                    if search.returncode == 0:
                        window = search.stdout.splitlines()[0]
                        break
                    if process.poll() is not None:
                        self.fail(f"Worminal exited: {process.communicate()[1].decode()}")
                    time.sleep(.05)
                self.assertIsNotNone(window, "Worminal did not map")
                subprocess.run(["xdotool", "windowunmap", window], env=env, check=True)
                gate.touch()
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline and not done.exists():
                    if process.poll() is not None:
                        self.fail(f"Worminal exited while hidden: {process.communicate()[1].decode()}")
                    time.sleep(.05)
                self.assertEqual(done.read_text() if done.exists() else None, "done",
                                 "PTY output stopped draining while the view was hidden")
            finally:
                if process.poll() is None:
                    process.terminate()
                process.communicate(timeout=2)
                gate.unlink(missing_ok=True)
                done.unlink(missing_ok=True)

    def test_default_build_uses_o3(self):
        commands = subprocess.check_output(["make", "-nB"], cwd=ROOT, text=True)
        compiled = [line for line in commands.splitlines() if line.endswith(("src/terminal/st.c", "src/x11/x.c"))]
        self.assertEqual(len(compiled), 2)
        self.assertTrue(all("-O3" in line.split() for line in compiled))

    def test_first_keystroke_reaches_shell(self):
        with isolated_display() as env:
            smoke_x11(env)

    def test_first_keystroke_latency_probe(self):
        with isolated_display() as env:
            sample = measure_one(env, 0)
            self.assertGreater(sample["echo"], sample["key"])
            self.assertLess(sample["map-request"], sample["fontconfig"])
            self.assertLess(sample["captured"], sample["echo"])

    def test_window_manager_placement(self):
        for geometry, expected in ((None, (100, 80)), ("80x24+37+53", (37, 53))):
            with self.subTest(geometry=geometry), isolated_display() as env:
                check_placement(env, geometry, expected)

    def test_compact_rows_keep_descenders(self):
        with isolated_display() as env:
            title = f"Worminal overlap {os.getpid()}"
            process = subprocess.Popen(
                [str(ROOT / "build/compact-worminal"), "-T", title, "-g", "10x2",
                 "-e", "/bin/sh", "-c", "printf '\\033[?25lg'; sleep 10"],
                env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
            )
            try:
                deadline = time.monotonic() + 5
                result = None
                while time.monotonic() < deadline:
                    windows = subprocess.run(
                        ["xdotool", "search", "--onlyvisible", "--name", title],
                        env=env, capture_output=True, text=True,
                    )
                    if windows.returncode == 0:
                        window = windows.stdout.splitlines()[0]
                        result = subprocess.run(
                            [str(ROOT / "build/overlap_probe"), window],
                            env=env, capture_output=True, text=True,
                        )
                        if result.returncode == 0:
                            break
                    if process.poll() is not None:
                        self.fail(f"Compact terminal exited: {process.communicate()[1].decode()}")
                    time.sleep(0.05)
                else:
                    self.fail(f"Descender did not reach the next row: {result.stdout if result else 'no window'}")
            finally:
                if process.poll() is None:
                    process.terminate()
                process.communicate(timeout=2)
