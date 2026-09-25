"""Run X11 checks on a display that cannot reach the desktop session."""

from contextlib import contextmanager
import os
from pathlib import Path
import select
import shutil
import subprocess
import time
import uuid


@contextmanager
def isolated_display():
    for tool in ("Xvfb", "xdotool"):
        if not shutil.which(tool):
            raise RuntimeError(f"Install {tool} to run the X11 checks")

    read_fd, write_fd = os.pipe()
    server = None
    workspace_env = None
    try:
        server = subprocess.Popen(
            ["Xvfb", "-displayfd", str(write_fd), "-screen", "0", "1024x768x24", "-nolisten", "tcp"],
            pass_fds=(write_fd,),
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
        )
        os.close(write_fd)
        write_fd = -1
        ready, _, _ = select.select([read_fd], [], [], 5)
        number = os.read(read_fd, 32).strip() if ready else b""
        if not number.isdigit():
            detail = server.poll()
            server.terminate()
            _, stderr = server.communicate(timeout=5)
            raise RuntimeError(
                f"Xvfb did not start a private display (exit status: {detail}): "
                f"{stderr.decode(errors='replace')[-500:]}")

        env = os.environ.copy()
        env["DISPLAY"] = f":{number.decode()}"
        env["WORMINAL_SHARED_SOCKET_SCOPE"] = f"test-{os.getpid()}-{uuid.uuid4().hex}"
        workspace_env = env
        env.pop("XAUTHORITY", None)
        # -displayfd reports the chosen display before Xvfb accepts clients.
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            if subprocess.run(["xdotool", "getdisplaygeometry"], env=env,
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0:
                break
            time.sleep(0.01)
        else:
            raise RuntimeError("Private Xvfb display never accepted connections")
        yield env
    finally:
        if workspace_env is not None:
            subprocess.run([str(Path(__file__).resolve().parent.parent / "worminald"), "--stop"],
                           env=workspace_env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                           timeout=3)
        os.close(read_fd)
        if write_fd != -1:
            os.close(write_fd)
        if server is not None:
            server.terminate()
            try:
                server.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                server.kill()
                server.communicate()
