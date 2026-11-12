"""Project-local PulseAudio for private browser media checks; never change desktop audio."""
import os
import shutil
import subprocess

from tools.debian_runtime import unpack
from tools.paths import BUILD

RUNTIME = BUILD / "dependencies/pulseaudio"


def setup():
    from tools.build import headless
    if headless() or shutil.which("pulseaudio"):
        return
    if not (RUNTIME / "usr/bin/pulseaudio").is_file():
        unpack(["pulseaudio"], RUNTIME)
    environment = {}
    subprocess.run([*command(environment), "--version"], env={**os.environ, **environment}, check=True)


def command(env):
    installed = shutil.which("pulseaudio")
    if installed:
        return [installed]
    executable = RUNTIME / "usr/bin/pulseaudio"
    if not executable.is_file():
        raise RuntimeError("Run manage.py setup-preview-audio for private browser media checks.")
    modules = list(RUNTIME.glob("usr/lib/**/modules/module-null-sink.so"))
    libraries = list(RUNTIME.glob("usr/lib/**/libpulsecore-*.so"))
    if len(modules) != 1 or len(libraries) != 1:
        raise RuntimeError("Missing PulseAudio modules; rerun manage.py setup-preview-audio.")
    env["PULSE_DLPATH"] = str(modules[0].parent)
    paths = [str(libraries[0].parent), str(modules[0].parent),
             env.get("LD_LIBRARY_PATH", os.environ.get("LD_LIBRARY_PATH", ""))]
    env["LD_LIBRARY_PATH"] = os.pathsep.join(path for path in paths if path)
    return [str(executable)]
