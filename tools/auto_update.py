"""Unattended daily update: fetch a branch into a private clone, gate on the full suite, then install."""
from contextlib import contextmanager
import fcntl
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

from tools.paths import BUILD, ROOT

BRANCH = "main"
PREFIX = "/usr/local"
CLONE = BUILD / "auto-update"
# Commit last installed; kept beside the clone so a failed night is retried, a successful one is not.
STAMP = BUILD / "auto-update.installed"
# What /usr/local was last built from, whichever checkout or clone installed it: the nightly job never downgrades it.
INSTALLED = Path(os.environ.get("XDG_STATE_HOME") or Path.home() / ".local/state") / "worminal/installed-commit"
UNIT = "worminal-auto-update"
SCHEDULE = "*-*-* 03:00:00"
UNIT_DIRECTORY = Path.home() / ".config/systemd/user"


def git(*args, cwd=CLONE):
    return subprocess.run(["git", *args], cwd=cwd, check=True, text=True,
                          stdout=subprocess.PIPE).stdout.strip()


def manage(*args, cwd=CLONE, sudo=False):
    if args[0] in ("check", "check-install"):
        from tools.test_runner import retire_worker
        from tools.update_storage import verification_environment
        with verification_environment(cwd) as env:
            process = subprocess.Popen([sys.executable, "manage.py", *args], cwd=cwd, env=env)
            try:
                return process.wait()
            finally:
                retire_worker(process, Path(env["WORMINAL_TEST_CHECKS_DIR"]))
    return subprocess.run([*(["sudo", "-n"] if sudo else []), sys.executable, "manage.py", *args],
                          cwd=cwd).returncode


@contextmanager
def exclusive(cwd):
    """One checked update per checkout: concurrent runs share tmpfs mounts, the build tree and the install."""
    (Path(cwd) / "build").mkdir(exist_ok=True)
    with open(Path(cwd) / "build/.update.lock", "w") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise SystemExit("Another update or check of this checkout is running; wait for it to finish.")
        yield


def publish(cwd=ROOT, prefix=PREFIX, installed=INSTALLED, checked=True):
    """Build, optionally run the full suite, publish, verify the installed result, and report live processes."""
    from tools.update_storage import release_stale
    steps = (("build",), *((("setup-check-runtime",), ("check",)) if checked else ()),
             ("install", "--prefix", prefix, "--skip-build"))
    if prefix == PREFIX:
        steps += (("check-install", "--installed", "--skip-build"),)
    with exclusive(cwd):
        release_stale(cwd)
        for args in steps:
            print(f"Update: {' '.join(args)}", flush=True)
            if code := manage(*args, cwd=cwd, sudo=args[0] == "install"):
                print(f"Update stopped at {args[0]}; see the preceding diagnostics.", flush=True)
                return code
    if prefix == PREFIX:
        from tools import running
        print("Update: live processes (v2 owners hand off by themselves; v1 daemons are never touched)", flush=True)
        running.report(prefix)
        installed.parent.mkdir(parents=True, exist_ok=True)
        installed.write_text(git("rev-parse", "HEAD", cwd=cwd) + "\n")
    return 0


def delegate(cwd, prefix):
    """The clone runs its own `update`: only the code being installed knows its own steps."""
    return subprocess.run([sys.executable, "manage.py", "update", "--check", "--prefix", prefix], cwd=cwd).returncode


def contains_installed(clone, tip, installed, checkout):
    """False when the installed build is not an ancestor of `tip` (installing it would roll work back)."""
    if not installed.is_file():
        return True
    commit = installed.read_text().strip()
    # The installed commit may exist only in the developer's checkout; bring it into the clone to compare.
    subprocess.run(["git", "fetch", "-q", str(checkout), commit], cwd=clone, capture_output=True)
    known = subprocess.run(["git", "cat-file", "-e", commit + "^{commit}"], cwd=clone).returncode == 0
    return not known or subprocess.run(["git", "merge-base", "--is-ancestor", commit, tip], cwd=clone).returncode == 0


def notify_failure(message):
    """Desktop notice when the user enabled update notifications; a missing tool or config is not an error."""
    try:
        config = json.loads(subprocess.run([str(Path(PREFIX) / "bin/worminald"), "config", "--json"],
                                           capture_output=True, text=True, check=True).stdout)
        if config.get("update_notifications") and shutil.which("notify-send"):
            subprocess.run(["notify-send", "--app-name=Worminal", "Worminal update failed", message], check=False)
    except (OSError, subprocess.CalledProcessError, ValueError):
        pass


def update(remote=None, branch=BRANCH, clone=CLONE, stamp=STAMP, prefix=PREFIX, installed=INSTALLED, checkout=ROOT):
    """Install the branch tip if it differs from the last install, contains the installed build, and passes."""
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
        if not contains_installed(clone, tip, installed, checkout):
            print(f"Skipping {tip[:7]}: it does not contain the installed build "
                  f"{installed.read_text().strip()[:7]}; push or merge that work to {branch}.")
            return 0
        git("reset", "-q", "--hard", tip, cwd=clone)
        if code := delegate(clone, prefix):
            print(f"Update to {tip[:7]} did not finish; the installed stamp was not advanced.")
            notify_failure(f"{tip[:7]} did not pass its checks; see journalctl --user -u {UNIT}.")
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
