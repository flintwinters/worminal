"""Deterministic combined-frontend checks; invoked only through manage.py."""

from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import os
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
        body = (f"<!doctype html><title>{title}</title><input autofocus>"
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
               "WEBKIT_DISABLE_DMABUF_RENDERER": "1", "G_DEBUG": "fatal-criticals"}


@contextmanager
def service(env):
    daemon = subprocess.Popen([str(ROOT / "worminald-preview"), "--serve"], env=env,
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
        subprocess.run([str(ROOT / "worminald-preview"), "--stop"], env=env,
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
                trial_env["WORMINAL_TEST_DAEMON"] = str(ROOT / "worminald-preview")
            log_path = ROOT / f".checks/preview-{backend}-{'remote' if remote else 'local'}.log"
            with service(trial_env), log_path.open("wb") as log:
                command = [str(ROOT / "build/preview_test")]
                if os.environ.get("WORMINAL_PREVIEW_DEBUG"):
                    command = ["gdb", "-batch", "-ex", "run", "-ex", "bt", "--args", *command]
                result = subprocess.run(["dbus-run-session", "--", *command],
                                        env=trial_env, stdout=log, stderr=log, timeout=90)
            output = log_path.read_text(errors="replace")
            if result.returncode or not any(line.startswith("Preview ") for line in output.splitlines()):
                print(output[-4000:], file=sys.stderr)
                return result.returncode or 1
            print(next((line for line in output.splitlines() if line.startswith("Preview ")), output[-500:]))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
