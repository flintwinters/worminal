"""Prepare an optional Debian Weston runtime entirely inside project test artifacts."""

from pathlib import Path
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent.parent
LOCAL = ROOT / ".checks/weston"
SUPPORTED_ABIS = (10, 16)


def setup():
    LOCAL.mkdir(parents=True, exist_ok=True)
    dependencies = subprocess.check_output(["apt-cache", "depends", "weston"], text=True)
    library = re.search(r"Depends: (libweston-\d+-\d+)", dependencies)
    if not library:
        raise ValueError("Install Weston using your system package manager; no Debian runtime package found")
    major = int(library[1].split("-")[1])
    if major not in SUPPORTED_ABIS:
        raise ValueError(f"The test seat supports Weston ABIs {SUPPORTED_ABIS}; found {major}")
    command = ["apt-get", "download", "weston", library[1], library[1].rsplit("-", 1)[0] + "-dev"]
    # Never mix stale packages or public struct layouts from different ABIs.
    root = LOCAL / library[1]
    with tempfile.TemporaryDirectory(prefix="download-", dir=LOCAL) as packages:
        subprocess.run(command, cwd=packages, check=True)
        for package in sorted(Path(packages).glob("*.deb")):
            subprocess.run(["dpkg-deb", "-x", str(package), str(root)], check=True)
    active = LOCAL / "active-library.pending"
    active.write_text(library[1] + "\n")
    active.replace(LOCAL / "active-library")
    print("Project-local Weston ready. Run: python3 manage.py check-preview-wayland")


def command(env):
    if env.get("WORMINAL_WESTON"):
        return [env["WORMINAL_WESTON"], "--backend=headless-backend.so", "--no-config"]
    active = LOCAL / "active-library"
    library = active.read_text().strip() if active.exists() else None
    if library and not re.fullmatch(r"libweston-\d+-\d+", library):
        raise ValueError("Invalid local Weston selection; rerun manage.py setup-preview-wayland")
    root = LOCAL / library if library else LOCAL / "root"
    executable = root / "usr/bin/weston"
    if not executable.exists():
        raise ValueError("Run manage.py setup-preview-wayland, or set WORMINAL_WESTON to a compositor with a seat")
    headers = list(root.glob("usr/include/libweston-*/libweston/libweston.h"))
    if len(headers) != 1:
        raise ValueError("Local Weston headers are absent or mixed; rerun manage.py setup-preview-wayland")
    include = headers[0].parent.parent
    major = int(include.name.split("-")[1])
    if major not in SUPPORTED_ABIS or (library and major != int(library.split("-")[1])):
        raise ValueError("Unsupported or mismatched Weston headers; rerun manage.py setup-preview-wayland")
    lib = next((root / "usr/lib").glob("*/weston/libexec_weston.so.0")).parent.parent
    backend = lib / f"libweston-{major}/headless-backend.so"
    env["LD_LIBRARY_PATH"] = f"{lib}:{lib / 'weston'}:" + env.get("LD_LIBRARY_PATH", "")
    from tools.build import pkg_flags, step, words
    module = ROOT / f"build/weston_seat-{major}.so"
    step(module, [*words("CC", "cc"), "-shared", "-fPIC", "-O2",
                  f"-I{include}", *pkg_flags("--cflags", ("pixman-1",)),
                  "tests/weston_seat.c", *pkg_flags("--libs", ("wayland-server",))],
         ["tests/weston_seat.c", headers[0], *([active] if active.exists() else [])], depfile=True)
    env["WESTON_MODULE_MAP"] = f"headless-backend.so={backend};desktop-shell.so={lib / 'weston/desktop-shell.so'};"
    env["WESTON_DATA_DIR"] = str(root / "usr/share/weston")
    client = root / "usr/libexec/weston-desktop-shell"
    if not client.exists():
        client = lib / "weston-desktop-shell"
    config = LOCAL / f"weston-{major}.ini"
    config.write_text(f"[shell]\nclient={client}\nlocking=false\npanel-position=none\n"
                      f"[input-method]\npath={client.parent / 'weston-keyboard'}\n")
    return [str(executable), "--backend=headless-backend.so", "--shell=desktop-shell.so",
            f"--config={config}", f"--modules={module}"]
