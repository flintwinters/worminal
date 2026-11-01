"""Prepare an optional private activation-test window manager without installing system packages."""
from pathlib import Path
import os
import shutil
import subprocess
from tools.debian_runtime import unpack

ROOT = Path(__file__).resolve().parent.parent
LOCAL = ROOT / ".checks/muffin"
RUNTIME = LOCAL / "root"


def setup():
    LOCAL.mkdir(parents=True, exist_ok=True)
    packages = ["muffin", "libmuffin0", "muffin-common", "libcinnamon-desktop4", "cinnamon-desktop-data",
                "cinnamon-settings-daemon"]
    unpack(packages, RUNTIME)
    schemas = RUNTIME / "usr/share/glib-2.0/schemas"
    # The Cinnamon schemas may extend installed GNOME schemas; compile one complete private catalogue.
    for source in Path("/usr/share/glib-2.0/schemas").glob("*.xml"):
        if not (schemas / source.name).exists(): shutil.copy2(source, schemas / source.name)
    subprocess.run(["glib-compile-schemas", "--strict", str(schemas)], check=True)
    environment = os.environ.copy()
    subprocess.run([*command(environment), "--version"], env=environment, check=True)
    print("Project-local Muffin ready. Run: python3 manage.py check-window-activation")


def available():
    return shutil.which("muffin") is not None or (RUNTIME / "usr/bin/muffin").is_file()


def command(env):
    installed = shutil.which("muffin")
    if installed: return [installed, "--sm-disable", "--x11"]
    executable = RUNTIME / "usr/bin/muffin"
    if not executable.is_file(): raise ValueError("Run manage.py setup-window-activation for private Muffin checks")
    libraries = next((RUNTIME / "usr/lib").glob("*-linux-gnu"))
    env["LD_LIBRARY_PATH"] = f"{libraries}:{libraries / 'muffin'}:" + env.get("LD_LIBRARY_PATH", "")
    env["GSETTINGS_SCHEMA_DIR"] = str(RUNTIME / "usr/share/glib-2.0/schemas")
    env["XDG_DATA_DIRS"] = str(RUNTIME / "usr/share") + ":" + env.get("XDG_DATA_DIRS", "/usr/local/share:/usr/share")
    # Muffin supports an absolute plugin filename; its compiled-in system directory is not this runtime.
    plugin = libraries / "muffin/plugins/libdefault.so"
    return [str(executable), "--sm-disable", "--x11", "--mutter-plugin=" + str(plugin)]
