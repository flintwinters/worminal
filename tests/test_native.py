"""Build settings and a real keyboard-to-PTY smoke check."""

import os
from pathlib import Path
import select
import subprocess
import time
import unittest

from x11 import isolated_display
from latency import measure_one


ROOT = Path(__file__).resolve().parent.parent


def smoke_x11(env):
    result_path = ROOT / ".checks" / "input"
    result_path.parent.mkdir(exist_ok=True)
    result_path.unlink(missing_ok=True)
    title = f"Worminal smoke {os.getpid()}"
    script = (
        '[ -n "$WINDOWID" ] && [ "$(stty size)" = "24 80" ] || exit 4; '
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
