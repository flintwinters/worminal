"""Preview protocol records, version coexistence and PTY backpressure limits."""

from functools import partial
from itertools import product
import hashlib
import json
import os
import re
import shlex
import signal
from pathlib import Path
import socket
import statistics
import struct
import subprocess
import tempfile
import time
import unittest
import uuid

from workspace_protocol import number, string, send as wire_send, wait_kind as wire_wait, packet as wire_packet
from perf_metrics import cpu_time, memory

ROOT = Path(__file__).resolve().parent.parent
DAEMON = ROOT / os.environ.get("WORMINAL_TEST_PREVIEW_DAEMON", "worminald-preview")
send = partial(wire_send, version=2)
wait_kind = partial(wire_wait, version=2)


def records(data):
    def integer():
        nonlocal data
        value = struct.unpack("!I", data[:4])[0]
        data = data[4:]
        return value

    def text():
        nonlocal data
        length = integer()
        value, data = data[:length].decode(), data[length:]
        return value

    count = integer()
    integer()
    integer()
    result = []
    for _ in range(count):
        tab, kind, url = integer(), integer(), text()
        mode, cursor, title, directory = integer(), integer(), text(), text()
        for _ in range(integer()):
            text()
        result.append((tab, kind, url, mode, cursor, title, directory))
    if data:
        raise AssertionError("extra catalog data")
    return result


@unittest.skipUnless(DAEMON.exists(), "run manage.py build-preview first")
class PreviewWorkspaceTest(unittest.TestCase):
    def test_pause_resume_and_disconnected_view_preserve_shell(self):
        tab = self.launch("stty raw -echo; printf READY; exec cat")
        self.daemon.send_signal(signal.SIGSTOP)
        try:
            send(self.peer, 3, tab, b"after-resume")
        finally:
            self.daemon.send_signal(signal.SIGCONT)
        output = b""
        while b"after-resume" not in output:
            _, chunk = wait_kind(self.peer, 14, tab)
            output += chunk
        self.peer.close()
        with self.connect() as peer:
            send(peer, 19)
            _, catalog = wait_kind(peer, 7)
            self.assertEqual([(row[0], row[1]) for row in records(catalog)], [(tab, 0)])
            send(peer, 4, tab, number(40) + number(4))
            wait_kind(peer, 10, tab)
            send(peer, 3, tab, b"still-alive")
            # Reattaching must preserve the original shell and accept new input.
            for _ in range(512):
                kind, ident, payload = wire_packet(peer, version=2)
                if kind == 9 and ident == tab:
                    text = ''.join(chr(struct.unpack_from('!I', payload, offset)[0])
                                   for offset in range(4, len(payload), 16))
                    if "still-alive" in text:
                        break
            else:
                self.fail("reattached shell did not echo input")
        self.assertIsNone(self.daemon.poll())

    def setUp(self):
        self.scope = f"preview-{os.getpid()}-{uuid.uuid4().hex}"
        self.env = {**os.environ, "WORMINAL_SHARED_SOCKET_SCOPE": self.scope, "WORMINAL_INHERIT_TEST": "inherited"}
        self.legacy = socket.socket(socket.AF_UNIX)
        self.legacy.bind(f"\0worminal-workspace-{os.getuid()}-{self.scope}")
        self.legacy.listen(1)
        self.legacy.settimeout(.05)
        self.addCleanup(self.legacy.close)
        self.daemon = subprocess.Popen([str(DAEMON), "--serve"], env=self.env,
                                       stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        self.addCleanup(self.stop)
        self.peer = self.connect()
        self.addCleanup(self.peer.close)

    def connect(self):
        deadline = time.monotonic() + 3
        while True:
            peer = socket.socket(socket.AF_UNIX)
            peer.settimeout(2)
            try:
                peer.connect(f"\0worminal-workspace-v2-{os.getuid()}-{self.scope}")
                return peer
            except OSError:
                peer.close()
                if time.monotonic() > deadline:
                    self.fail("preview service did not start")
                time.sleep(.01)

    def stop(self):
        self.daemon.terminate()
        self.daemon.communicate(timeout=3)

    def start_tab(self, command):
        args = ["/bin/sh", "-c", command]
        launch = b"".join((number(40), number(4), number(1), number(len(args)), number(0),
                           string("test"), string(str(ROOT)), string(""), string("-"), string(""),
                           *(string(arg) for arg in args)))
        send(self.peer, 1, payload=launch)
        tab, _ = wait_kind(self.peer, 20)
        wait_kind(self.peer, 7)
        return tab

    def launch(self, command):
        tab = self.start_tab(command)
        output = b""
        marker = b"READY:inherited" if "WORMINAL_INHERIT_TEST" in command else b"READY"
        while marker not in output:
            _, chunk = wait_kind(self.peer, 14, tab)
            output += chunk
        if "WORMINAL_INHERIT_TEST" in command:
            self.assertIn(b"READY:inherited", output)
        return tab

    def test_browser_records_have_no_pty_and_saved_links_survive_attach(self):
        send(self.peer, 17, payload=string("https://example.invalid/first"))
        tab, _ = wait_kind(self.peer, 20)
        _, catalog = wait_kind(self.peer, 7)
        self.assertEqual(records(catalog)[0][:3], (tab, 1, "https://example.invalid/first"))
        children = Path(f"/proc/{self.daemon.pid}/task/{self.daemon.pid}/children").read_text().strip()
        self.assertEqual(children, "")
        # Terminal-only operations cannot manufacture a PTY for a browser record.
        send(self.peer, 4, tab, number(100) + number(20))
        send(self.peer, 3, tab, b"ignored")
        send(self.peer, 6, tab, number(1) + number(3))
        with self.connect() as peer:
            send(peer, 19)
            _, catalog = wait_kind(peer, 7)
            self.assertEqual(len(records(catalog)), 1)
            send(peer, 18, tab, string("https://example.invalid/second"))
            _, catalog = wait_kind(self.peer, 7)
            self.assertEqual(records(catalog)[0][2], "https://example.invalid/second")
        terminal = self.launch("stty raw -echo; printf READY; exec cat")
        send(self.peer, 5, tab)
        _, catalog = wait_kind(self.peer, 7)
        self.assertEqual([(row[0], row[1]) for row in records(catalog)], [(terminal, 0)])
        with self.assertRaises(socket.timeout):
            self.legacy.accept()

    def test_remote_environment_and_deferred_input_order(self):
        command = ('stty raw -echo; printf "READY:$WORMINAL_INHERIT_TEST"; sleep .1; '
                   'dd bs=1024 count=64 iflag=fullblock 2>/dev/null | sha256sum; sleep 1')
        tab = self.launch(command)
        import hashlib
        payload = bytes(range(256)) * 256
        send(self.peer, 3, tab, payload)
        output = b""
        digest = hashlib.sha256(payload).hexdigest().encode()
        while digest not in output:
            _, chunk = wait_kind(self.peer, 14, tab)
            output += chunk
        self.assertIn(digest, output)

    def test_command_converts_calling_terminal_and_preserves_order(self):
        cases = (("https://example.invalid/converted", "https://example.invalid/converted"),
                 ("about:blank", "about:blank"), (None, "worminal://" + ROOT.as_uri()[7:]))
        for executable, (url, expected) in product((DAEMON, ROOT / "worminal"), cases):
            with self.subTest(executable=executable.name, url=url):
                self.assert_browser_conversion(executable, url, expected)

    def assert_browser_conversion(self, executable, target, expected, directory=ROOT):
        command = "cd " + shlex.quote(str(directory)) + " && " + shlex.quote(str(executable)) + " --new-browser-tab"
        if target is not None: command += " " + shlex.quote(target)
        tab = self.launch("stty raw -echo; printf READY; dd bs=1 count=1 >/dev/null 2>&1; "
                          + command + "; sleep 30")
        other = self.launch("stty raw -echo; printf READY; exec cat")
        send(self.peer, 3, tab, b"x")
        for _ in range(512):
            _, payload = wait_kind(self.peer, 7)
            rows = records(payload)
            converted = next((row for row in rows if row[0] == tab and row[1] == 1), None)
            if converted:
                self.assertEqual(converted[2], expected)
                identities = [row[0] for row in rows]
                self.assertLess(identities.index(tab), identities.index(other))
                self.assertEqual(next(row for row in rows if row[0] == other)[1], 0)
                break
        else:
            self.fail("calling terminal was not converted")
        send(self.peer, 3, other, b"still-live")
        output = b""
        while b"still-live" not in output:
            _, chunk = wait_kind(self.peer, 14, other)
            output += chunk

    def test_browser_command_encodes_paths_in_the_callers_working_directory(self):
        checks = ROOT / ".checks"
        checks.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="browser-path- #?% ü", dir=checks) as temporary:
            directory = Path(temporary)
            special = directory / "page #?% ü.html"
            special.write_text("<title>Local page</title>")
            plain = directory / "plain.html"
            plain.write_text("<title>Plain page</title>")
            cases = ((None, "worminal://" + directory.as_uri()[7:]),
                     (str(special), special.as_uri()), (special.name, special.as_uri()),
                     ("./" + special.name, special.as_uri()), (plain.name, plain.as_uri()),
                     (special.as_uri(), special.as_uri()),
                     ("missing.html", (directory / "missing.html").as_uri()))
            for executable, (target, expected) in product((DAEMON, ROOT / "worminal"), cases):
                with self.subTest(executable=executable.name, target=target):
                    self.assert_browser_conversion(executable, target, expected, directory)

    def test_browser_command_outside_a_terminal_is_rejected(self):
        for executable in (DAEMON, ROOT / "worminal"):
            with self.subTest(executable=executable.name):
                env = {key: value for key, value in self.env.items()
                       if key not in ("DISPLAY", "WAYLAND_DISPLAY")}
                result = subprocess.run([str(executable), "--new-browser-tab"], env=env,
                                        capture_output=True, timeout=5)
                self.assertEqual(result.returncode, 1)
                self.assertIn(b"Open worminal", result.stderr)
                self.assertIn(b"keep WORMINAL_SHARED_SOCKET_SCOPE unchanged", result.stderr)
                self.assertNotIn(b"Invalid browser URL", result.stderr)

    def test_empty_browser_url_keeps_the_calling_terminal(self):
        command = shlex.quote(str(DAEMON)) + " --new-browser-tab ''"
        tab = self.launch("stty raw -echo; printf READY; dd bs=1 count=1 >/dev/null 2>&1; "
                          + command + "; exec cat")
        send(self.peer, 3, tab, b"x")
        output = b""
        while b"Invalid browser URL" not in output:
            _, chunk = wait_kind(self.peer, 14, tab)
            output += chunk
        send(self.peer, 19)
        _, payload = wait_kind(self.peer, 7)
        self.assertEqual([(row[0], row[1]) for row in records(payload)], [(tab, 0)])
        self.assertNotIn(b"Open worminal", output)

    def test_input_reply_frame_precedes_its_mode_catalog(self):
        # Catalog titles are also frontend readiness barriers. A client must not
        # see a new input mode while its imported alternate screen is stale.
        code = ("import os\nos.write(1,b'READY')\n"
                "for i in range(12):\n"
                " os.read(0,1)\n"
                " mode = 'h' if i%2==0 else 'l'\n"
                " os.write(1,f'\\x1b[?1049{mode}\\x1b]2;mode-{i}\\a'.encode())\n"
                "os.read(0,1)\n")
        tab = self.launch("stty raw -echo; exec python3 -u -c " + shlex.quote(code))
        alternate = 0
        for index in range(12):
            send(self.peer, 3, tab, b"x")
            for _ in range(512):
                kind, identity, payload = wire_packet(self.peer, version=2)
                if identity == tab and kind == 8:
                    alternate = struct.unpack("!6I", payload)[5]
                if kind == 7 and records(payload)[0][5] == f"mode-{index}":
                    self.assertEqual(alternate, int(index % 2 == 0),
                                     "mode catalog preceded its corresponding frame")
                    break
            else:
                self.fail("missing mode catalog")

    def test_stale_resize_cannot_reclaim_another_controller(self):
        tab = self.launch("stty raw -echo; printf READY; exec cat")
        with self.connect() as peer:
            send(peer, 19)
            wait_kind(peer, 7)
            send(peer, 4, tab, number(40) + number(4))
            wait_kind(peer, 10, tab)
            wait_kind(self.peer, 15, tab)
            send(self.peer, 23, tab, number(39) + number(4))
            send(self.peer, 19)
            wait_kind(self.peer, 7)  # Same-stream barrier after the stale allocation.
            send(peer, 19)
            kind, _, _ = wire_packet(peer, version=2)
            self.assertEqual(kind, 7, "stale resize released the new controller")

    def test_new_tab_output_frame_latency(self):
        # A separately paced tab keeps the shared batching clock active.
        background = ("import os,time\nos.write(1,b'READY')\nwhile True:\n"
                      " os.write(1,b'\\x1b[H.'); time.sleep(.004)\n")
        self.launch(f"exec python3 -u -c {shlex.quote(background)}")
        samples, frames, count = [], 0, 40
        started_cpu = cpu_time(self.daemon.pid)
        for _ in range(count):
            tab = self.start_tab("printf READY; exec cat")
            started, visible, row_text, stream = None, False, b"", b""
            for _ in range(10000):
                kind, identity, payload = wire_packet(self.peer, version=2)
                observed = time.monotonic_ns()
                if identity != tab:
                    continue
                if kind == 14:
                    stream += payload
                    if b"READY" in stream and started is None:
                        started = observed
                elif kind == 9 and len(payload) >= 84 and struct.unpack_from("!I", payload)[0] == 0:
                    row_text = bytes(struct.unpack_from("!I", payload, 4 + 16 * x)[0] for x in range(5))
                    visible = row_text == b"READY"
                elif kind == 10:
                    frames += 1
                    if visible and started is not None:
                        samples.append((observed - started) / 1e6)
                        break
            else:
                self.fail(f"new terminal {tab} output missing: print={started is not None}, row={row_text!r}")
            send(self.peer, 5, tab)
        self.assertEqual(len(samples), count)
        ordered = sorted(samples)
        result = {"samples_ms": samples, "n": len(samples), "frames": frames,
                  "median_ms": statistics.median(samples), "p95_ms": ordered[(95 * len(samples) + 99) // 100 - 1],
                  "daemon_sha256": hashlib.sha256(DAEMON.read_bytes()).hexdigest(),
                  "daemon_cpu_ms": (cpu_time(self.daemon.pid) - started_cpu) * 1000,
                  "cpu_resolution_ms": 1000 / os.sysconf("SC_CLK_TCK"),
                  "daemon_memory": memory({self.daemon.pid}),
                  "notes": "New tab first PTY print to complete matching frame; another tab writes every 4 ms. "
                           "Includes no GTK paint, keyboard input or physical display."}
        label = os.environ.get("WORMINAL_FRAME_PERF_NAME", "candidate")
        path = ROOT / ".checks" / f"new-tab-output-latency-{label}.json"
        path.parent.mkdir(exist_ok=True)
        path.write_text(json.dumps(result, indent=2) + "\n")
        print(f"{label} new-tab output to frame: median {result['median_ms']:.2f} ms, "
              f"p95 {result['p95_ms']:.2f} ms; {path.relative_to(ROOT)}")

    def test_paced_output_frame_latency(self):
        # The second write arrives near the previous frame's deadline. No
        # WIRE_INPUT is sent: this measures the ordinary output batching path.
        count = 40
        code = ("import os,time\nos.write(1,b'READY')\n"
                f"for i in range({count}):\n"
                " time.sleep(.030)\n os.write(1,f'\\x1b[HS{i:03d}A'.encode())\n"
                " time.sleep(.014)\n os.write(1,f'\\x1b[HS{i:03d}B'.encode())\n"
                "os.read(0,1)\n")
        tab = self.launch("stty raw -echo; exec python3 -u -c " + shlex.quote(code))
        pending, samples, stream, visible = {}, [], b"", b""
        started_cpu, frames = cpu_time(self.daemon.pid), 0
        while True:
            kind, identity, payload = wire_packet(self.peer, version=2)
            observed = time.monotonic_ns()
            if identity != tab:
                continue
            if kind == 14:
                stream += payload
                matches = list(re.finditer(rb"\x1b\[H(S\d{3}B)", stream))
                for match in matches:
                    pending[match[1]] = observed
                stream = stream[matches[-1].end():] if matches else stream[-16:]
            elif kind == 9 and len(payload) >= 84 and struct.unpack_from("!I", payload)[0] == 0:
                visible = bytes(struct.unpack_from("!I", payload, 4 + 16 * x)[0] for x in range(5))
            elif kind == 10:
                frames += 1
                if visible in pending:
                    samples.append((observed - pending.pop(visible)) / 1e6)
                    if visible == f"S{count - 1:03d}B".encode():
                        break
        self.assertTrue(samples)
        ordered = sorted(samples)
        result = {"samples_ms": samples, "n": len(samples), "writes": count,
                  "frames": frames, "daemon_cpu_ms": (cpu_time(self.daemon.pid) - started_cpu) * 1000,
                  "cpu_resolution_ms": 1000 / os.sysconf("SC_CLK_TCK"),
                  "daemon_memory": memory({self.daemon.pid}),
                  "median_ms": statistics.median(samples), "p95_ms": ordered[(95 * len(samples) + 99) // 100 - 1]}
        label = os.environ.get("WORMINAL_FRAME_PERF_NAME", "candidate")
        directory = ROOT / ".checks"
        directory.mkdir(exist_ok=True)
        path = directory / f"frame-output-latency-{label}.json"
        path.write_text(json.dumps(result, indent=2) + "\n")
        print(f"{label} PTY output to complete frame: median {result['median_ms']:.2f} ms, "
              f"p95 {result['p95_ms']:.2f} ms (n={len(samples)}/{count}); {path.relative_to(ROOT)}")
        print(f"  {frames} frames; daemon CPU {result['daemon_cpu_ms']:.1f} ms; "
              f"PSS {result['daemon_memory']['pss_kib']} KiB")

    def test_queue_saturation_stops_only_affected_tab(self):
        blocked = self.launch("stty raw -echo; printf READY; exec sleep 30")
        for _ in range(18):
            send(self.peer, 3, blocked, b"x" * 65536)
        wait_kind(self.peer, 16, blocked)
        _, catalog = wait_kind(self.peer, 7)
        self.assertEqual(records(catalog), [])
        tab = self.launch("stty raw -echo; printf READY; exec cat")
        send(self.peer, 3, tab, b"responsive")
        output = b""
        while b"responsive" not in output:
            _, chunk = wait_kind(self.peer, 14, tab)
            output += chunk
        self.assertIsNone(self.daemon.poll())


class ServiceDiagnosticsTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        checks = ROOT / ".checks"
        checks.mkdir(exist_ok=True)
        cls.directory = tempfile.TemporaryDirectory(prefix="service-fault-", dir=checks)
        cls.addClassCleanup(cls.directory.cleanup)
        cls.library = Path(cls.directory.name) / "fault.so"
        subprocess.run(["cc", "-shared", "-fPIC", "-Wall", "-Wextra", "-o", str(cls.library),
                        str(ROOT / "tests/service_fault.c"), "-ldl"], check=True)

    def environment(self):
        return {**os.environ, "LD_PRELOAD": str(self.library),
                "WORMINAL_SHARED_SOCKET_SCOPE": f"diagnostics-{uuid.uuid4().hex}"}

    def test_event_loop_failure_records_operation_and_errno(self):
        result = subprocess.run([str(DAEMON), "--serve"],
                                env={**self.environment(), "WORMINAL_TEST_PSELECT_FAILURE": "1"},
                                capture_output=True, text=True, timeout=3)
        self.assertEqual(result.returncode, 1)
        self.assertIn("JOURNAL[5] Workspace service started (protocol 2)", result.stderr)
        self.assertIn("JOURNAL[3] workspace pselect failed:", result.stderr)
        self.assertIn("(errno 9)", result.stderr)
        self.assertIn("JOURNAL[3] Workspace service exited with status 1", result.stderr)

    def test_fatal_pty_failure_is_reported_to_journal_and_stderr(self):
        env = self.environment()
        service = subprocess.Popen([str(DAEMON), "--serve"], env=env,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        try:
            deadline = time.monotonic() + 3
            with socket.socket(socket.AF_UNIX) as peer:
                while True:
                    try:
                        peer.connect(f"\0worminal-workspace-v2-{os.getuid()}-"
                                     + env['WORMINAL_SHARED_SOCKET_SCOPE'])
                        break
                    except ConnectionRefusedError:
                        if time.monotonic() >= deadline:
                            self.fail("diagnostic daemon did not start")
                        time.sleep(.01)
                launch = b"".join((number(40), number(4), number(1), number(0), number(0),
                                   string("test"), string(str(ROOT)), string(""), string(""), string("")))
                send(peer, 1, payload=launch)
                _, errors = service.communicate(timeout=3)
            self.assertEqual(service.returncode, 1)
            self.assertIn(b"JOURNAL[3] openpty failed: Too many open files", errors)
            self.assertEqual(errors.count(b"openpty failed: Too many open files"), 2)
        finally:
            if service.poll() is None:
                service.terminate()
            service.communicate(timeout=3)
