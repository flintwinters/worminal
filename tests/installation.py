"""Verify staged or published artifacts and native module resolution on private displays."""
from runtime import CHECKS


import hashlib
import argparse
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import time
import xml.etree.ElementTree as ET

from preview import fixture, profile
from workspace_protocol import service_pid
from x11 import close_window, isolated_display

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
from tools import build
from tools.theme import theme_values


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main(installed=False):
    stage = Path('/') if installed else CHECKS / "install-stage"
    previous = os.environ.get("DESTDIR")
    os.environ["DESTDIR"] = str(stage)
    try:
        if not installed:
            build.install(Path("/usr/local"), skip_build=True)
        files = build.installation_files("/usr/local")
    finally:
        if previous is None:
            os.environ.pop("DESTDIR", None)
        else:
            os.environ["DESTDIR"] = previous
    records = []
    for source, target, mode in files:
        expected = (source.read_text().replace("VERSION", build.VERSION).encode()
                    if source.suffix == ".1" else source.read_bytes())
        if target.read_bytes() != expected or target.stat().st_mode & 0o777 != mode:
            raise AssertionError(f"Staged artifact differs from verified build: {target}")
        records.append({"path": str(target.relative_to(stage)), "sha256": digest(target)})
    prefix = stage / "usr/local"
    # A private Fontconfig search path catches missing/corrupt font payloads even when
    # the developer already has the default family installed elsewhere.
    fontconfig = ET.Element('fontconfig')
    ET.SubElement(fontconfig, 'dir').text = str(prefix / 'share/fonts/worminal')
    ET.SubElement(fontconfig, 'cachedir').text = str(CHECKS / 'install-font-cache')
    fontconfig_path = CHECKS / 'install-fontconfig.xml'
    ET.ElementTree(fontconfig).write(fontconfig_path, encoding='utf-8', xml_declaration=True)
    family = theme_values({})[0]['font_family']
    for style, filename in (('Regular', build.FONT_FILES[0]), ('Italic', build.FONT_FILES[1])):
        matched = subprocess.check_output(['fc-match', '-f', '%{file}', f'{family}:style={style}'],
            env={**os.environ, 'FONTCONFIG_FILE': str(fontconfig_path)}, text=True, timeout=10)
        if Path(matched) != prefix / 'share/fonts/worminal' / filename:
            raise AssertionError(f'Bundled {style} font did not resolve: {matched}')
    executable = prefix / "bin/worminal"
    for name, option, status in (("worminal", "--version", 0),):
        result = subprocess.run([str(prefix / "bin" / name), option],
                                capture_output=True, text=True, timeout=5)
        if result.returncode != status or build.VERSION not in result.stdout + result.stderr:
            raise AssertionError(f"Staged {name} version failed: {result.stderr}")
    for name in ("worminald",):
        result = subprocess.run([str(prefix / "bin" / name), "--invalid"],
                                capture_output=True, text=True, timeout=5)
        if result.returncode != 2 or "--serve|--bridge|--stop" not in result.stderr:
            raise AssertionError(f"Staged {name} CLI validation failed: {result.stderr}")
    env = {**os.environ, "WORMINAL_TEST_EXECUTABLE": str(executable),
           "WORMINAL_TEST_DAEMON": str(prefix / "bin/worminald")}
    subprocess.run([sys.executable, "-m", "unittest", "discover", "-s", "tests",
                    "-p", "test_master_startup.py", "-q"], env=env, check=True, timeout=60)
    title = "Staged browser verification"
    with isolated_display() as display, profile(display) as env, fixture() as url:
        env.update(GDK_BACKEND="x11", GTK_USE_PORTAL="0")
        env.pop("WAYLAND_DISPLAY", None)
        env.pop("DBUS_SESSION_BUS_ADDRESS", None)
        with (CHECKS / "install-browser.log").open("wb") as log:
            owner = subprocess.Popen([str(executable), "-T", title, "--surf", url],
                                     env=env, stdout=log, stderr=log, start_new_session=True)
            try:
                frontend = prefix / "lib/worminal" / build.RUST_FRONTEND_MODULE
                browser = prefix / "lib/worminal" / build.RUST_BROWSER_MODULE
                browser_digest = digest(browser)
                deadline = time.monotonic() + 15
                while time.monotonic() < deadline:
                    if owner.poll() is not None:
                        raise AssertionError("Staged browser exited; see build/checks/install-browser.log")
                    maps = Path(f"/proc/{owner.pid}/maps").read_text()
                    # The Rust loader pins each opened inode across atomic replacement. Verify actual
                    # resident descriptor bytes and mapped identity, not merely a pathname in /proc/maps.
                    browser_loaded = False
                    if str(browser) in maps:
                        for descriptor in Path(f"/proc/{owner.pid}/fd").iterdir():
                            try:
                                info, expected = descriptor.stat(), browser.stat()
                                if (info.st_dev == expected.st_dev and info.st_ino == expected.st_ino
                                        and digest(descriptor) == browser_digest
                                        and any(str(info.st_ino) == line.split()[4] and str(browser) in line
                                                for line in maps.splitlines())):
                                    browser_loaded = True
                                    break
                            except FileNotFoundError:
                                continue
                    visible = subprocess.run(["xdotool", "search", "--onlyvisible", "--pid", str(owner.pid)],
                                             env=env, capture_output=True, timeout=2)
                    if str(frontend) in maps and browser_loaded and visible.returncode == 0:
                        # Mapping the lazy module precedes profile/view creation. Wait for the real
                        # WebKit document, otherwise a loaded-library check can accept an unusable browser.
                        windows = [value.decode() for value in visible.stdout.splitlines()]
                        ready = [window for window in windows if subprocess.check_output(
                            ['xdotool', 'getwindowname', window], env=env, text=True).strip() == 'Fixture']
                        if ready:
                            browser_window = ready[0]
                            break
                    time.sleep(.02)
                else:
                    (CHECKS / "install-browser-maps.log").write_text(maps)
                    raise AssertionError("Staged browser did not map verified modules and a visible window")
                with socket.socket(socket.AF_UNIX) as connection:
                    connection.settimeout(3)
                    connection.connect(f"\0worminal-workspace-v2-{os.getuid()}-"
                                       f"{env['WORMINAL_SHARED_SOCKET_SCOPE']}")
                    daemon_pid = service_pid(connection)
                daemon = prefix / "bin/worminald"
                if Path(f"/proc/{daemon_pid}/exe").resolve() != daemon:
                    raise AssertionError("Staged launcher started a daemon from another installation")
                if not (Path(env['XDG_DATA_HOME']) / 'worminal-preview').is_dir():
                    raise AssertionError("Release browser did not retain the established profile directory")
                if (Path(env['XDG_DATA_HOME']) / 'worminal-rust').exists():
                    raise AssertionError("Release browser used the candidate profile")
                subprocess.run(['xdotool', 'windowfocus', '--sync', browser_window,
                                'type', '--clearmodifiers', 'staged'], env=env, check=True, timeout=5)
                deadline = time.monotonic() + 5
                while subprocess.check_output(['xdotool', 'getwindowname', browser_window],
                                              env=env, text=True).strip() != 'Edited:staged':
                    if time.monotonic() >= deadline:
                        raise AssertionError('Staged browser did not accept keyboard input')
                    time.sleep(.02)
                close_window(env, 'Edited:staged')
                if owner.wait(timeout=10) != 0:
                    raise AssertionError("Staged browser did not close gracefully")
            finally:
                # This group was created above, on this private display and private profile only.
                try:
                    os.killpg(owner.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    owner.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(owner.pid, signal.SIGKILL)
                    owner.wait(timeout=5)
                subprocess.run([str(prefix / "bin/worminald"), "--stop"], env=env,
                               stdout=log, stderr=log, timeout=5)
    report = 'production-install-verification.json' if installed else 'install-verification.json'
    (CHECKS / report).write_text(json.dumps(records, indent=2) + "\n")
    print(f"{'Published' if installed else 'Staged'} /usr/local: hashes, modes, native CLI, SSH input, "
          "daemon and lazy module paths passed.")
    return 0


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--installed', action='store_true',
                        help='Read published artifacts; never install or overwrite.')
    raise SystemExit(main(parser.parse_args().installed))
