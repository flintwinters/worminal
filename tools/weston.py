"""Prepare an optional Debian Weston runtime entirely inside project test artifacts."""

from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parent.parent
LOCAL = ROOT / ".checks/weston"


def setup():
    packages = LOCAL / "packages"
    packages.mkdir(parents=True, exist_ok=True)
    dependencies = subprocess.check_output(["apt-cache", "depends", "weston"], text=True)
    library = re.search(r"Depends: (libweston-\d+-\d+)", dependencies)
    if not library:
        raise ValueError("Install Weston using your system package manager; no Debian runtime package found")
    if library[1] != "libweston-10-0":
        raise ValueError("The project-local virtual seat is pinned to Weston 10 (Debian 12)")
    command = ["apt-get", "download", "weston", library[1], library[1].replace("-0", "-dev")]
    subprocess.run(command, cwd=packages, check=True)
    for package in sorted(packages.glob("*.deb")):
        subprocess.run(["dpkg-deb", "-x", str(package), str(LOCAL / "root")], check=True)
    print("Project-local Weston ready. Run: python3 manage.py check-preview-wayland")


def command(env):
    executable = LOCAL / "root/usr/bin/weston"
    if env.get("WORMINAL_WESTON"):
        return [env["WORMINAL_WESTON"], "--backend=headless-backend.so", "--no-config"]
    if not executable.exists():
        raise ValueError("Run manage.py setup-preview-wayland, or set WORMINAL_WESTON to a compositor with a seat")
    lib = next((LOCAL / "root/usr/lib").glob("*/weston/libexec_weston.so.0")).parent.parent
    backend = next(lib.glob("libweston-*/headless-backend.so"))
    env["LD_LIBRARY_PATH"] = f"{lib}:{lib / 'weston'}:" + env.get("LD_LIBRARY_PATH", "")
    from tools.build import pkg_flags, step, words
    module = ROOT / "build/weston_seat.so"
    step(module, [*words("CC", "cc"), "-shared", "-fPIC", "-O2",
                  f"-I{LOCAL / 'root/usr/include/libweston-10'}", *pkg_flags("--cflags", ("pixman-1",)),
                  "tests/weston_seat.c", *pkg_flags("--libs", ("wayland-server",))], ["tests/weston_seat.c"])
    env["WESTON_MODULE_MAP"] = f"headless-backend.so={backend};desktop-shell.so={lib / 'weston/desktop-shell.so'};"
    env["WESTON_DATA_DIR"] = str(LOCAL / "root/usr/share/weston")
    config = LOCAL / "weston.ini"
    config.write_text(f"[shell]\nclient={lib / 'weston-desktop-shell'}\nlocking=false\npanel-position=none\n")
    return [str(executable), "--backend=headless-backend.so", "--shell=desktop-shell.so",
            f"--config={config}", f"--modules={module}"]
