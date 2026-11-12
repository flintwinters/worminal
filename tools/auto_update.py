"""Unattended daily update: fetch a branch into a private clone, gate on the full suite, then install."""
import fcntl
import os
from pathlib import Path
import subprocess
import sys

from tools.paths import BUILD, ROOT

BRANCH = "main"
PREFIX = "/usr/local"
CLONE = BUILD / "auto-update"
# Commit last installed; kept beside the clone so a failed night is retried, a successful one is not.
STAMP = BUILD / "auto-update.installed"
UNIT = "worminal-auto-update"
SCHEDULE = "*-*-* 03:00:00"
UNIT_DIRECTORY = Path.home() / ".config/systemd/user"


def git(*args, cwd=CLONE):
    return subprocess.run(["git", *args], cwd=cwd, check=True, text=True,
                          stdout=subprocess.PIPE).stdout.strip()


def manage(*args, cwd=CLONE, sudo=False):
    if args[0] in ("check", "check-install"):
        from tools.update_storage import verification_environment
        with verification_environment(cwd) as env:
            return subprocess.run([sys.executable, "manage.py", *args], cwd=cwd, env=env).returncode
    return subprocess.run([*(["sudo", "-n"] if sudo else []), sys.executable, "manage.py", *args],
                          cwd=cwd).returncode


def publish(cwd=ROOT, prefix=PREFIX):
    """Build and validate as the user; elevate only publication and verify the result."""
    steps = (("build",), ("setup-preview-audio",), ("check",),
             ("install", "--prefix", prefix, "--skip-build"))
    if prefix == PREFIX:
        steps += (("check-install", "--installed", "--skip-build"),)
    for args in steps:
        print(f"Update: {' '.join(args)}", flush=True)
        if code := manage(*args, cwd=cwd, sudo=args[0] == "install"):
            print(f"Update stopped at {args[0]}; see the preceding diagnostics.", flush=True)
            return code
    return 0


def update(remote=None, branch=BRANCH, clone=CLONE, stamp=STAMP, prefix=PREFIX):
    """Install the branch tip if it differs from the last install and passes build and check."""
    remote = remote or git("remote", "get-url", "origin", cwd=ROOT)
    clone.parent.mkdir(parents=True, exist_ok=True)
    with open(clone.parent / ".auto-update.lock", "w") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            print("Another update is running.")
            return 0
        if not (clone / ".git").exists():
            subprocess.run(["git", "clone", "-q", "--branch", branch, remote, str(clone)], check=True)
        git("fetch", "-q", remote, branch, cwd=clone)
        tip = git("rev-parse", "FETCH_HEAD", cwd=clone)
        if stamp.exists() and stamp.read_text() == tip:
            print(f"Already installed {tip[:7]}.")
            return 0
        git("reset", "-q", "--hard", tip, cwd=clone)
        if code := publish(cwd=clone, prefix=prefix):
            print(f"Update to {tip[:7]} did not finish; the installed stamp was not advanced.")
            return code
        stamp.write_text(tip)
        print(f"Installed {tip[:7]}.")
        return 0


def units():
    """Return the systemd user service and timer texts; Persistent catches up a missed 03:00."""
    service = f"""[Unit]
Description=Update Worminal from {BRANCH}

[Service]
Type=oneshot
Environment=PATH={os.environ['PATH']}
ExecStart={sys.executable} {ROOT / 'manage.py'} auto-update
"""
    timer = f"""[Unit]
Description=Update Worminal daily

[Timer]
OnCalendar={SCHEDULE}
Persistent=true

[Install]
WantedBy=timers.target
"""
    return {f"{UNIT}.service": service, f"{UNIT}.timer": timer}


def systemctl(*args):
    subprocess.run(["systemctl", "--user", *args], check=True)


def schedule(remove=False, directory=UNIT_DIRECTORY):
    if remove:
        systemctl("disable", "--now", f"{UNIT}.timer")
        for name in units():
            (directory / name).unlink(missing_ok=True)
        return systemctl("daemon-reload")
    directory.mkdir(parents=True, exist_ok=True)
    for name, text in units().items():
        (directory / name).write_text(text)
    systemctl("daemon-reload")
    systemctl("enable", "--now", f"{UNIT}.timer")
