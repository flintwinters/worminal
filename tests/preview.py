"""Deterministic combined-frontend checks; invoked only through manage.py."""

from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import os
import select
import signal
from pathlib import Path
import shutil
import socket
import ssl
import subprocess
import sys
import tempfile
import threading
import time

from x11 import isolated_display
from workspace_protocol import send as wire_send, wait_kind as wire_wait

ROOT = Path(__file__).resolve().parent.parent


class Fixture(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/download":
            self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Content-Disposition", 'attachment; filename="fixture.txt"')
            self.end_headers()
            self.wfile.write(b"Worminal download fixture\n")
            return
        title = "Other" if self.path == "/other" else "Fixture"
        body = (f"<!doctype html><title>{title}</title>"
                "<input autofocus oninput=\"document.title='Edited:'+this.value\">"
                '<a href="/other" target="_blank">Popup</a> '
                '<a href="/download" download>Download</a>').encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args):
        pass


@contextmanager
def fixture(tls=False):
    server = ThreadingHTTPServer(("127.0.0.1", 0), Fixture)
    if tls:
        certificates = ROOT / ".checks/preview-tls"
        certificates.mkdir(exist_ok=True)
        cert, key = certificates / "certificate.pem", certificates / "key.pem"
        subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
                        "-subj", "/CN=localhost", "-keyout", str(key), "-out", str(cert)],
                       check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(cert, key)
        server.socket = context.wrap_socket(server.socket, server_side=True)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"{'https' if tls else 'http'}://127.0.0.1:{server.server_port}"
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


@contextmanager
def profile(env):
    checks = ROOT / ".checks"
    checks.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="preview-", dir=checks) as directory:
        base = Path(directory)
        runtime = base / "runtime"
        runtime.mkdir(mode=0o700)
        yield {**env, "XDG_RUNTIME_DIR": str(runtime), "XDG_DATA_HOME": str(base / "data"),
               "XDG_CACHE_HOME": str(base / "cache"), "NO_AT_BRIDGE": "1", "SHELL": "/bin/sh",
               # Browser GPU helpers can finish cache writes after the UI process exits.
               "MESA_SHADER_CACHE_DISABLE": "true",
               "WEBKIT_DISABLE_DMABUF_RENDERER": "1", "G_DEBUG": "fatal-criticals"}


@contextmanager
def service(env):
    daemon = subprocess.Popen([str(ROOT / "worminald"), "--serve"], env=env,
                              stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    try:
        address = f"\0worminal-workspace-v2-{os.getuid()}-{env['WORMINAL_SHARED_SOCKET_SCOPE']}"
        deadline = time.monotonic() + 5
        while True:
            with socket.socket(socket.AF_UNIX) as peer:
                try:
                    peer.connect(address)
                    break
                except OSError:
                    if daemon.poll() is not None or time.monotonic() > deadline:
                        raise RuntimeError("Preview service did not start")
                    time.sleep(.01)
        yield
    finally:
        subprocess.run([str(ROOT / "worminald"), "--stop"], env=env,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=3)
        try:
            _, errors = daemon.communicate(timeout=3)
        except subprocess.TimeoutExpired:
            daemon.kill()
            _, errors = daemon.communicate()
        if errors:
            print(errors.decode(errors="replace")[-2000:], file=sys.stderr)


@contextmanager
def wayland_display():
    import uuid

    sys.path.insert(0, str(ROOT))
    from tools.weston import command

    weston_env = dict(os.environ)
    weston_command = command(weston_env)
    if not shutil.which(weston_command[0]):
        raise RuntimeError("Install Weston, or set WORMINAL_WESTON to its executable; X11 fallback is forbidden")
    with profile(weston_env) as env:
        env.pop("DISPLAY", None)
        env.pop("WAYLAND_DISPLAY", None)
        env.pop("DBUS_SESSION_BUS_ADDRESS", None)
        env.update(GDK_BACKEND="wayland", WAYLAND_DISPLAY="worminal-test",
                   WORMINAL_SHARED_SOCKET_SCOPE=f"wayland-{os.getpid()}-{uuid.uuid4().hex}")
        log_path = ROOT / ".checks/preview-weston.log"
        with log_path.open("wb") as log:
            compositor = subprocess.Popen([*weston_command, "--use-pixman",
                                           "--socket=worminal-test", "--idle-time=0"],
                                          env=env, stdout=log, stderr=log)
            try:
                deadline = time.monotonic() + 5
                while not (Path(env["XDG_RUNTIME_DIR"]) / "worminal-test").exists():
                    if compositor.poll() is not None or time.monotonic() > deadline:
                        raise RuntimeError(f"Headless Weston failed; see {log_path}")
                    time.sleep(.02)
                yield env
            finally:
                compositor.terminate()
                compositor.wait(timeout=5)


@contextmanager
def qtile_display():
    with isolated_display() as env:
        log_path = ROOT / ".checks/preview-qtile.log"
        with log_path.open("wb") as log:
            manager = subprocess.Popen(
                ["qtile", "start", "--backend", "x11", "--no-spawn", "--config",
                 str(ROOT / "tests/fixtures/preview_qtile.py"), "--socket", str(ROOT / ".checks/preview-qtile.sock"),
                 "--log-path", str(log_path)], env=env, stdout=log, stderr=log)
            try:
                deadline = time.monotonic() + 10
                while True:
                    result = subprocess.run(["xprop", "-root", "_NET_SUPPORTING_WM_CHECK"], env=env,
                                            capture_output=True, text=True)
                    if "window id #" in result.stdout:
                        break
                    if manager.poll() is not None or time.monotonic() > deadline:
                        raise RuntimeError(f"Qtile did not start; see {log_path}")
                    time.sleep(.05)
                yield env
            finally:
                manager.terminate()
                manager.wait(timeout=5)


def startup_check(env, log):
    """Invalid displays and arguments must not start SSH or create a service tab."""
    failed = {**env, "DISPLAY": "worminal-invalid", "WAYLAND_DISPLAY": "worminal-invalid",
              "WORMINAL_TRACE_TIMING": "1", "WORMINAL_TEST_BRIDGE_DELAY": "30",
              "WORMINAL_TEST_DAEMON": str(ROOT / "worminald"),
              "PATH": str(ROOT / "tests/fixtures/preview-master") + os.pathsep + env["PATH"]}
    for arguments, expected, started in ((["-e", "/bin/cat"], 1, False),
                                         (["--invalid-startup-option"], 2, False)):
        trial = dict(failed)
        if expected == 2:
            for key in ("DISPLAY", "WAYLAND_DISPLAY"):
                trial.pop(key, None)
                if key in env:
                    trial[key] = env[key]
        process = subprocess.Popen([str(ROOT / "worminal"), "--master", "preview-test", *arguments],
                                   env=trial, stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
        try:
            _, errors = process.communicate(timeout=5)
            log.write(errors)
            log.flush()
            if process.returncode != expected or (b"worminal-timing ssh-started " in errors) != started:
                raise RuntimeError(f"Invalid-display/argument startup failed: status={process.returncode}, {errors!r}")
            if expected == 1 and b"cannot open display" not in errors.lower():
                raise RuntimeError("The initial SSH probe did not reach the expected display failure")
            try:
                os.killpg(process.pid, 0)
            except ProcessLookupError:
                pass
            else:
                raise RuntimeError("Failed startup left an SSH helper running")
        finally:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait(timeout=5)
        with socket.socket(socket.AF_UNIX) as peer:
            peer.settimeout(3)
            peer.connect(f"\0worminal-workspace-v2-{os.getuid()}-{env['WORMINAL_SHARED_SOCKET_SCOPE']}")
            # Two ordered round trips also let already-readable bridge traffic
            # be processed before accepting an empty-catalog observation.
            for _ in range(2):
                wire_send(peer, 19, version=2)
                _, catalog = wire_wait(peer, 7, version=2)
                if len(catalog) < 12 or int.from_bytes(catalog[:4], "big"):
                    raise RuntimeError("Failed GUI startup created an orphan service tab")


def broker_check(env, url, log):
    """Independent launches must share the GTK owner and the live DOM, not reload a URL."""
    executable = str(ROOT / "worminal")
    title = f"Broker terminal {os.getpid()}"
    first = subprocess.Popen([executable, "-T", title, "-o", "-", "-e", "/bin/sh", "-c",
                              "stty raw -echo; printf BROKER_READY; exec cat"],
                             env=env, stdout=subprocess.PIPE, stderr=log)

    def read(stream, marker):
        output = b""
        deadline = time.monotonic() + 5
        while marker not in output and time.monotonic() < deadline:
            ready, _, _ = select.select([stream], [], [], max(0, deadline - time.monotonic()))
            if ready:
                chunk = os.read(stream.fileno(), 4096)
                if not chunk: break
                output += chunk
        if marker not in output: raise RuntimeError(f"Missing broker output: {marker!r}")

    def window(name):
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            result = subprocess.run(["xdotool", "search", "--onlyvisible", "--name", "^" + name + "$"],
                                    env=env, capture_output=True, text=True)
            if result.returncode == 0: return result.stdout.splitlines()[0]
            time.sleep(.01)
        raise RuntimeError(f"Missing broker window: {name}")

    def key(*args):
        subprocess.run(["xdotool", *args], env=env, check=True, stdout=subprocess.DEVNULL, stderr=log)

    try:
        read(first.stdout, b"BROKER_READY")
        mappings = Path(f"/proc/{first.pid}/maps").read_text()
        if "frontend.so" not in mappings:
            raise RuntimeError("Window owner did not load the GTK frontend module")
        if "libwebkit2gtk" in mappings:
            raise RuntimeError("Terminal-only frontend loaded WebKit before opening a browser")
        terminal_window = window(title)
        invalid = subprocess.run([executable, "--invalid"], env=env,
                                 stdout=subprocess.PIPE, stderr=log, timeout=5)
        if invalid.returncode == 0 or first.poll() is not None:
            raise RuntimeError("Invalid forwarded arguments terminated the shared frontend")
        result = subprocess.run([executable, "--surf", url + "/page"], env=env,
                                stdout=subprocess.PIPE, stderr=log, timeout=10)
        if result.returncode: raise RuntimeError("Second launch did not join the frontend")
        browser_window = window("Fixture")
        if "libwebkit2gtk" not in Path(f"/proc/{first.pid}/maps").read_text():
            raise RuntimeError("Browser tab did not load its WebKit module")
        key("windowfocus", "--sync", browser_window)
        key("key", "--clearmodifiers", "ctrl+k")
        key("type", "--clearmodifiers", "--delay", "0", url + "/other")
        key("key", "--clearmodifiers", "Return")
        window("Other")
        key("key", "--clearmodifiers", "alt+Left")
        window("Fixture")
        key("key", "--clearmodifiers", "alt+Right")
        window("Other")
        key("key", "--clearmodifiers", "alt+Left")
        window("Fixture")
        key("key", "--clearmodifiers", "ctrl+k", "Escape")
        key("type", "--clearmodifiers", "--delay", "0", "shared-state")
        window("Edited:shared-state")
        key("windowfocus", "--sync", terminal_window)
        key("key", "--clearmodifiers", "ctrl+9")
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            name = subprocess.check_output(["xdotool", "getwindowname", terminal_window], env=env, text=True).strip()
            if name == "Edited:shared-state": break
            time.sleep(.01)
        else: raise RuntimeError("Browser state was not shared with the independently launched window")
        key("type", "--clearmodifiers", "--delay", "0", "!")
        window("Edited:shared-state!")
        key("key", "--clearmodifiers", "ctrl+t")
        window("Worminal")
        forwarded_env = {**env, "WORMINAL_BROKER_TEST": "caller-value"}
        forwarded = subprocess.Popen([executable, "-T", "Forwarded terminal", "-o", "-", "-e", "/bin/sh", "-c",
                                      "printf 'FORWARDED:%s:%s' \"$WORMINAL_BROKER_TEST\" \"$PWD\"; exec cat"],
                                     cwd=ROOT / "tests", env=forwarded_env, stdout=subprocess.PIPE, stderr=log)
        try:
            if forwarded.wait(timeout=5): raise RuntimeError("Forwarded terminal launch failed")
            read(forwarded.stdout, f"FORWARDED:caller-value:{ROOT / 'tests'}".encode())
        finally:
            forwarded.stdout.close()
    finally:
        first.terminate(); first.communicate(timeout=5)


def main():
    backend = sys.argv[1] if len(sys.argv) == 2 else "x11"
    display = {"x11": isolated_display, "wayland": wayland_display, "qtile": qtile_display}[backend]
    with display() as display_env, profile(display_env) as env, fixture() as url, fixture(tls=True) as tls_url:
        if backend == "wayland":
            # Preserve the compositor's socket directory, while isolating browser storage.
            env["XDG_RUNTIME_DIR"] = display_env["XDG_RUNTIME_DIR"]
        env["GDK_BACKEND"] = "wayland" if backend == "wayland" else "x11"
        env["WORMINAL_PREVIEW_FIXTURE"] = url
        env["WORMINAL_PREVIEW_TLS_FIXTURE"] = tls_url
        env["GTK_USE_PORTAL"] = "0"
        if backend == "qtile":
            env["WORMINAL_PREVIEW_TILED"] = "1"
        env.pop("DBUS_SESSION_BUS_ADDRESS", None)
        for remote in (False, True):
            trial_env = dict(env)
            if remote:
                trial_env["PATH"] = str(ROOT / "tests/fixtures/preview-master") + os.pathsep + env["PATH"]
                trial_env["WORMINAL_PREVIEW_TEST_MASTER"] = "preview-test"
                trial_env["WORMINAL_TEST_DAEMON"] = str(ROOT / "worminald")
            log_path = ROOT / f".checks/preview-{backend}-{'remote' if remote else 'local'}.log"
            with service(trial_env), log_path.open("wb") as log:
                startup_check(trial_env, log)
                command = [str(ROOT / "build/preview_test")]
                if os.environ.get("WORMINAL_PREVIEW_DEBUG"):
                    command = ["gdb", "-batch", "-ex", "run", "-ex", "bt", "--args", *command]
                result = subprocess.run(["dbus-run-session", "--", *command],
                                        env=trial_env, stdout=log, stderr=log, timeout=90)
                if not result.returncode and backend == "x11" and not remote:
                    broker_check(trial_env, url, log)
            output = log_path.read_text(errors="replace")
            if result.returncode or not any(line.startswith("Preview ") for line in output.splitlines()):
                print(output[-4000:], file=sys.stderr)
                return result.returncode or 1
            print(next((line for line in output.splitlines() if line.startswith("Preview ")), output[-500:]))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
