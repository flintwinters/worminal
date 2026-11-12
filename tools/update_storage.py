"""Ownership-correct private verification storage for updates on UID-remapping filesystems."""
from contextlib import contextmanager
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


@contextmanager
def verification_environment(cwd):
    checks = Path(cwd) / "build/checks"
    checks.mkdir(parents=True, exist_ok=True)
    directory = Path(tempfile.mkdtemp(prefix="update-posix-", dir=checks))
    if directory.stat().st_uid == os.geteuid():
        directory.rmdir()
        yield os.environ.copy()
        return
    # NFS can map every newly created file to a different uid. Publication tests must
    # exercise the real ownership checks, so give them real Unix storage, never mocks.
    print(f"Update: using ownership-correct temporary storage at {directory}", flush=True)
    subprocess.run(["sudo", "-n", "mount", "-t", "tmpfs", "-o",
                    f"uid={os.geteuid()},gid={os.getegid()},mode=0700,size=2G",
                    "tmpfs", str(directory)], check=True)
    try:
        yield {**os.environ, "WORMINAL_TEST_CHECKS_DIR": str(directory)}
    finally:
        # Preserve diagnostics on the underlying filesystem before releasing memory.
        saved = directory.with_name(directory.name + "-saved")
        shutil.copytree(directory, saved, symlinks=True)
        subprocess.run(["sudo", "-n", "umount", str(directory)], check=True)
        directory.rmdir()
        print(f"Update diagnostics: {saved}", flush=True)
