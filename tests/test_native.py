"""Build settings and a real keyboard-to-PTY smoke check."""

import os
import hashlib
from pathlib import Path
import select
import subprocess
import time
import unittest

from x11 import isolated_display
from latency import measure_one


ROOT = Path(__file__).resolve().parent.parent


def check_placement(env, geometry, expected):
    title = f"Worminal placement {os.getpid()} {'explicit' if geometry else 'default'}"
    manager = subprocess.Popen(
        [str(ROOT / ".checks/placement_wm")], env=env,
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
    result_path = ROOT / ".checks" / "input"
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
            search = subprocess.run(["xdotool", "search", "--onlyvisible", "--name", title], env=env, capture_output=True, text=True)
            if search.returncode == 0:
                window = search.stdout.splitlines()[0]
                break
            if process.poll() is not None:
                raise AssertionError(f"Worminal exited before opening a window: {process.communicate()[1].decode()}")
            time.sleep(0.05)
        if window is None:
            raise AssertionError("Worminal did not open an X11 window")

        subprocess.run([str(ROOT / ".checks/icon_probe"), window], env=env, check=True)

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
            border = subprocess.run([str(ROOT / ".checks/border_probe"), window],
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
    def test_shared_view_end_to_end(self):
        with isolated_display() as env:
            env = {**env, "WORMINAL_SHARED_SOCKET_SCOPE": f"proof-{os.getpid()}"}
            title = f"Worminal shared {os.getpid()}"
            received = ROOT / ".checks" / "shared_input"
            received.unlink(missing_ok=True)
            script = (
                "printf '\033[?25l'; "
                'while IFS= read -r line; do '
                'printf "[%s]\\n" "$line"; '
                'printf "%s\\n" "$line" >> "$1"; done'
            )
            owner = subprocess.Popen(
                [str(ROOT / "worminal"), "-S", "-T", title, "-e", "/bin/sh",
                 "-c", script, "sh", str(received)],
                env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
            )

            def windows(count):
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline:
                    found = subprocess.run(
                        ["xdotool", "search", "--onlyvisible", "--name", title],
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
                subprocess.run(["xdotool", "windowfocus", "--sync", window], env=env, check=True)
                time.sleep(.1)

            def typed(value):
                subprocess.run(["xdotool", "type", "--clearmodifiers", "--delay", "0", value],
                               env=env, check=True)
                subprocess.run(["xdotool", "key", "Return"], env=env, check=True)
                deadline = time.monotonic() + 3
                while time.monotonic() < deadline:
                    if received.exists() and value in received.read_text().splitlines():
                        return
                    time.sleep(.05)
                self.fail(f"Input {value!r} did not reach the shared shell")

            def image_hash(window):
                pixels = subprocess.check_output(["xwd", "-id", window, "-silent"], env=env)
                return hashlib.sha256(pixels).digest()

            try:
                first = windows(1)[0]
                subprocess.run([str(ROOT / "worminal"), "-S"], env=env,
                               capture_output=True, check=True, timeout=5)
                first, second = windows(2)
                focus(second)
                before = image_hash(second)
                focus(first)
                typed("alpha")
                self.assertEqual(image_hash(second), before,
                                 "inactive view received a PTY repaint")
                focus(second)
                self.assertNotEqual(image_hash(second), before,
                                    "reactivated view did not catch up")
                typed("beta")
                subprocess.run(["xdotool", "windowclose", first], env=env, check=True)
                self.assertEqual(windows(1), [second], "first view did not close")
                self.assertIsNone(owner.poll(), "first close ended the shared owner")
                focus(second)
                typed("gamma")
                self.assertEqual(received.read_text().splitlines(), ["alpha", "beta", "gamma"])
                subprocess.run(["xdotool", "windowclose", second], env=env, check=True)
                try:
                    owner.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    visible = subprocess.run(
                        ["xdotool", "search", "--onlyvisible", "--name", title],
                        env=env, capture_output=True, text=True,
                    )
                    ready, _, _ = select.select([owner.stderr], [], [], 0)
                    trace = os.read(owner.stderr.fileno(), 4096).decode() if ready else ""
                    self.fail(f"Owner remained after last close; windows={visible.stdout!r}, trace={trace!r}")
                self.assertEqual(owner.returncode, 0)
            finally:
                if owner.poll() is None:
                    owner.terminate()
                owner.wait(timeout=2)
                owner.stderr.close()
                received.unlink(missing_ok=True)

    def test_view_contexts_draw_independently(self):
        with isolated_display() as env:
            subprocess.run([str(ROOT / ".checks/view_state_test")], env=env, check=True)

    def test_unmapped_window_keeps_draining_pty(self):
        with isolated_display() as env:
            gate = ROOT / ".checks" / "drain_gate"
            done = ROOT / ".checks" / "drain_done"
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
        compile_commands = [line for line in commands.splitlines() if line.endswith(("-c st.c", "-c x.c"))]
        self.assertEqual(len(compile_commands), 2)
        self.assertTrue(all("-O3" in line.split() for line in compile_commands))

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
                [str(ROOT / ".checks/compact-worminal"), "-T", title, "-g", "10x2",
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
                            [str(ROOT / ".checks/overlap_probe"), window],
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
