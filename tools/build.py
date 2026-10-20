"""Native build, analysis, and installation; invoked through manage.py."""

import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess

from tools.theme import generate_theme


ROOT = Path(__file__).resolve().parent.parent
VERSION = "0.9.3"
COMMON = ("terminal/defaults.c", "terminal/st.c", "terminal/session_view.c",
          "workspace/wire.c", "workspace/workspace_socket.c")
CLIENT = (*COMMON, "workspace/workspace_client.c", "x11/x.c", "x11/url.c")
SERVER = (*COMMON, "workspace/workspace_service.c")
PREVIEW = (*COMMON, "workspace/workspace_client.c", "gtk/app.c", "gtk/terminal.c",
           "gtk/browser_loader.c", "gtk/tabs.c", "gtk/windows.c")
PREVIEW_PACKAGES = ("gtk+-3.0", "pangocairo", "fontconfig", "freetype2", "webkit2gtk-4.1")
TERMINAL_PACKAGES = ("gtk+-3.0", "pangocairo", "fontconfig", "freetype2", "gmodule-2.0")
SOURCES = tuple(dict.fromkeys((*CLIENT, *SERVER)))
CPPFLAGS = ("-I.", "-Isrc", "-Isrc/terminal", "-Isrc/x11", "-Isrc/workspace",
            f'-DVERSION="{VERSION}"', "-D_XOPEN_SOURCE=600")
PACKAGES = ("x11", "xft", "fontconfig", "freetype2")
PROBES = {"key_injector": ("x11", "xtst"), "placement_wm": ("x11",),
          "overlap_probe": ("x11",), "border_probe": ("x11",), "icon_probe": ("x11",)}


def words(name, default=""):
    return shlex.split(os.environ.get(name, default))


def invoke(command):
    subprocess.run(command, cwd=ROOT, check=True)


def pkg_flags(option, packages=PACKAGES):
    return shlex.split(subprocess.check_output(
        [*words("PKG_CONFIG", "pkg-config"), option, *packages], cwd=ROOT, text=True))


def compile_flags():
    return [*pkg_flags("--cflags"), *CPPFLAGS, *words("CPPFLAGS"), *words("CFLAGS", "-O3")]


def link_flags():
    return ["-lm", "-lrt", "-lutil", *pkg_flags("--libs"), *words("LDFLAGS")]


def step(output, command, dependencies, *, depfile=False):
    """Publish successful outputs only; include command changes and compiler-discovered headers in freshness."""
    output = ROOT / output
    record = output.with_suffix(output.suffix + ".command.json")
    dep = output.with_suffix(output.suffix + ".d")
    dependencies = [ROOT / path for path in dependencies]
    if depfile and dep.exists():
        # GCC escapes whitespace in Make-format dependency paths; shlex decodes those escapes.
        dependencies += [ROOT / path for path in shlex.split(dep.read_text().replace("\\\n", "").split(":", 1)[1])]
    if (output.exists() and record.exists() and (not depfile or dep.exists())
            and json.loads(record.read_text()) == command
            and all(path.exists() and path.stat().st_mtime_ns <= output.stat().st_mtime_ns
                    for path in dependencies)):
        return
    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = output.with_name(output.name + ".pending")
    pending_dep = dep.with_name(dep.name + ".pending")
    args = [*command, "-o", str(temporary)]
    if depfile:
        args += ["-MMD", "-MF", str(pending_dep), "-MT", str(output)]
    print(f"Build: {output.relative_to(ROOT)}", flush=True)
    try:
        invoke(args)
        temporary.replace(output)
        if depfile:
            pending_dep.replace(dep)
        record.write_text(json.dumps(command))
    finally:
        temporary.unlink(missing_ok=True)
        pending_dep.unlink(missing_ok=True)


class Builder:
    def __init__(self):
        self.cc = words("CC", "cc")
        self.cflags = compile_flags()
        self.libs = link_flags()

    def program(self, output, sources, *, variant="default", flags=(), libs=None):
        objects = []
        for source in sources:
            obj = f"build/obj/{variant}/{Path(source).with_suffix('.o')}"
            step(obj, [*self.cc, *self.cflags, *flags, "-c", source], [source], depfile=True)
            objects.append(obj)
        step(output, [*self.cc, *objects, *(self.libs if libs is None else libs)], objects)

    def target(self, target):
        if target == "build/browser.so":
            old_flags = self.cflags
            self.cflags = [*CPPFLAGS, *words("CPPFLAGS"), *words("CFLAGS", "-O3"),
                           *pkg_flags("--cflags", PREVIEW_PACKAGES)]
            try:
                self.program(target, ["src/gtk/browser.c"], variant="browser", flags=["-fPIC"],
                             libs=["-shared", *pkg_flags("--libs", PREVIEW_PACKAGES), *words("LDFLAGS")])
            finally:
                self.cflags = old_flags
        elif target in ("worminal-x11", "worminald-x11"):
            sources = CLIENT if target == "worminal-x11" else SERVER
            self.program(target, [f"src/{source}" for source in sources],
                         flags=['-DWORKSPACE_DAEMON="worminald-x11"'])
        elif target in ("worminal", "worminald", "worminal-preview", "worminald-preview", "build/preview_test"):
            if target in ("worminal", "worminal-preview"):
                self.target("build/browser.so")
            flags = ["-DWORMINAL_PREVIEW_PROTOCOL"]
            server = target in ("worminald", "worminald-preview")
            sources = SERVER if server else PREVIEW
            if target == "build/preview_test":
                sources = ()
                flags += ["-O1", "-g"]
            libs = ["-lm", "-lrt", "-lutil", *words("LDFLAGS")]
            old_flags = self.cflags
            # Keep GTK, WebKit and X11 out of the preview daemon's build/link flags.
            self.cflags = [*CPPFLAGS, *words("CPPFLAGS"), *words("CFLAGS", "-O3")]
            if not server:
                packages = PREVIEW_PACKAGES if target == "build/preview_test" else TERMINAL_PACKAGES
                self.cflags += pkg_flags("--cflags", packages)
                libs += pkg_flags("--libs", packages)
            try:
                self.program(target, [f"src/{source}" for source in sources] or
                             ["tests/preview.c", *(f"src/{source}" for source in COMMON),
                              "src/workspace/workspace_client.c"],
                             variant="preview-service" if server else "preview-test" if not sources else "preview",
                             flags=flags, libs=libs)
            finally:
                self.cflags = old_flags
        elif target in ("build/compact-worminal", "build/worminald"):
            generate_theme(ROOT / "tests/fixtures/alacritty/compact.toml", ROOT / "build/compact_theme.h")
            flags = ['-DWORMINAL_THEME_HEADER="build/compact_theme.h"']
            if target == "build/compact-worminal":
                self.target("build/worminald")
            sources = CLIENT if target == "build/compact-worminal" else SERVER
            self.program(target, [f"src/{source}" for source in sources], variant="compact", flags=flags)
        elif target.removeprefix("build/") in PROBES and target.startswith("build/"):
            name = target.removeprefix("build/")
            packages = PROBES[name]
            step(target, [*self.cc, "-O2", *pkg_flags("--cflags", packages), f"tests/{name}.c",
                          *pkg_flags("--libs", packages)], [f"tests/{name}.c"])
        elif target == "build/view_state_test":
            # view_state.c includes x.c to exercise internal state; compile that translation unit once.
            sources = ["tests/view_state.c", "src/terminal/defaults.c", "src/x11/url.c", "src/workspace/wire.c",
                       "src/workspace/workspace_socket.c", "src/workspace/workspace_client.c"]
            self.program(target, sources, variant="view", flags=["-ffunction-sections", "-fdata-sections"],
                         libs=["-Wl,--gc-sections", *self.libs, *pkg_flags("--libs", ("xtst",))])
        elif target in ("build/scrollback_test", "build/url_test"):
            scrollback = target == "build/scrollback_test"
            sources = ["tests/scrollback.c"] if scrollback else ["tests/url.c", "src/x11/url.c"]
            flags = ["-O1", "-g", "-fsanitize=address,undefined"]
            if scrollback:
                flags += ["-ffunction-sections", "-fdata-sections"]
            self.program(target, sources, variant=Path(target).name, flags=flags,
                         libs=["-fsanitize=address,undefined", "-Wl,--gc-sections", "-lm"])
        else:
            raise ValueError(f"Unknown build target: {target}")


def build_targets(*targets):
    generate_theme()
    builder = Builder()
    for target in dict.fromkeys(("worminal", "worminald", *targets)):
        builder.target(target)


def analyze():
    generate_theme()
    tidy = [*words("CLANG_TIDY", "clang-tidy"), "-quiet"]
    invoke([*tidy, *(f"src/{source}" for source in SOURCES), "--", *compile_flags()])
    flags = [*CPPFLAGS, "-DWORMINAL_PREVIEW_PROTOCOL", *words("CPPFLAGS")]
    invoke([*tidy, "src/workspace/workspace_service.c", "--", *flags])
    invoke([*tidy, "src/gtk/app.c", "src/gtk/terminal.c", "src/gtk/browser.c", "src/gtk/browser_loader.c",
            "src/gtk/tabs.c", "src/gtk/windows.c", "--",
            *flags, *pkg_flags("--cflags", PREVIEW_PACKAGES)])


def clean():
    # Own only build outputs: preserve .checks logs and captures used to diagnose failures.
    if (ROOT / "build").exists():
        shutil.rmtree(ROOT / "build")
    for name in ("worminal", "worminald", "worminal-x11", "worminald-x11", "worminal-preview", "worminald-preview"):
        for suffix in ("", ".command.json", ".pending"):
            (ROOT / (name + suffix)).unlink(missing_ok=True)


def installation_files(prefix=None):
    prefix = Path(prefix if prefix is not None else os.environ.get("PREFIX", "/usr/local"))
    manprefix = Path(os.environ.get("MANPREFIX", str(prefix / "share/man")))
    if not prefix.is_absolute() or not manprefix.is_absolute():
        raise ValueError("PREFIX and MANPREFIX must be absolute paths")
    staging = Path(os.environ.get("DESTDIR", "/"))
    files = [(name, prefix / "bin" / name, 0o755)
             for name in ("worminal", "worminald", "worminal-x11", "worminald-x11")]
    files += [("docs/man/worminal.1", manprefix / "man1/worminal.1", 0o644),
              ("build/browser.so", prefix / "lib/worminal/browser.so", 0o755),
              ("assets/worminal.desktop", prefix / "share/applications/worminal.desktop", 0o644),
              ("assets/worminal.svg", prefix / "share/icons/hicolor/scalable/apps/worminal.svg", 0o644)]
    return [(ROOT / source, staging / str(destination).lstrip("/"), mode) for source, destination, mode in files]


def install(prefix=None):
    files = installation_files(prefix)
    build_targets("worminal-x11", "worminald-x11")
    for source, destination, mode in files:
        destination.parent.mkdir(parents=True, exist_ok=True)
        temporary = destination.with_name(destination.name + ".pending")
        try:
            if source.suffix == ".1":
                temporary.write_text(source.read_text().replace("VERSION", VERSION))
            else:
                shutil.copyfile(source, temporary)
            temporary.chmod(mode)
            # Running clients may still map the old executable or browser module.
            temporary.replace(destination)
        finally:
            temporary.unlink(missing_ok=True)
    print("Installed binaries, manual, desktop launcher, and icon.")


def uninstall(prefix=None):
    for _, destination, _ in installation_files(prefix):
        destination.unlink(missing_ok=True)
    print("Removed installed Worminal files.")
