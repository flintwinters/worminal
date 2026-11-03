"""Unpack optional graphical test dependencies into project-owned directories."""
from pathlib import Path
import subprocess
import tempfile


def unpack(packages, destination):
    destination.parent.mkdir(parents=True, exist_ok=True)
    # Each download has a fresh directory: never extract stale packages from another runtime ABI.
    with tempfile.TemporaryDirectory(prefix="download-", dir=destination.parent) as directory:
        subprocess.run(["apt-get", "download", *packages], cwd=directory, check=True)
        for package in sorted(Path(directory).glob("*.deb")):
            subprocess.run(["dpkg-deb", "-x", str(package), str(destination)], check=True)
