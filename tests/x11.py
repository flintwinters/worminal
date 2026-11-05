"""Run X11 checks on a display that cannot reach the desktop session."""

from contextlib import ExitStack, contextmanager
import os
import select
import shutil
import subprocess
import time
import uuid

from workspace_protocol import stop_workspace


def close_window(env, title, timeout=5):
    """Close one shared-owner window without XKillClient terminating all its peers."""
    from Xlib import X, display, protocol

    window = subprocess.check_output(
        ["xdotool", "search", "--onlyvisible", "--name", f"^{title}$"],
        env=env, text=True, timeout=timeout).splitlines()[0]
    connection = display.Display(env["DISPLAY"])
    try:
        client = connection.create_resource_object("window", int(window))
        client.send_event(protocol.event.ClientMessage(window=client,
            client_type=connection.intern_atom("WM_PROTOCOLS"),
            data=(32, [connection.intern_atom("WM_DELETE_WINDOW"), X.CurrentTime, 0, 0, 0])))
        connection.sync()
    finally:
        connection.close()


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
        try:
            # Services outlive windows and may execute a copied runtime. Wait for their exit before
            # fixture deletion: NFS retains mapped/unlinked executables as .nfs files until then.
            with ExitStack() as cleanup:
                if workspace_env is not None:
                    for version in (1, 2):
                        cleanup.callback(stop_workspace, workspace_env, version=version)
        finally:
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
