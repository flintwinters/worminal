"""Native build, analysis, and installation; invoked through manage.py."""

import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess

from tools.theme import generate_theme


ROOT = Path(__file__).resolve().parent.parent
VERSION = next(line.split('"')[1] for line in (ROOT / "src/version.h").read_text().splitlines()
               if line.startswith("#define WORMINAL_VERSION "))
BROWSER_ABI = next(line.split()[2] for line in (ROOT / "src/gtk/browser_module.h").read_text().splitlines()
                   if line.startswith("#define WORMINAL_BROWSER_ABI "))
BROWSER_MODULE = f"browser-v{BROWSER_ABI}.so"
RUST_BROWSER_MODULE = next(line.split('"')[1]
                          for line in (ROOT / "src/gtk/rust_browser_module.h").read_text().splitlines()
                          if line.startswith("#define WORMINAL_RUST_BROWSER_FILE "))
RUST_FRONTEND_MODULE = next(line.split('"')[1]
                           for line in (ROOT / "src/gtk/rust_frontend_module.h").read_text().splitlines()
                           if line.startswith("#define WORMINAL_RUST_FRONTEND_FILE "))
ENGINE_DEFAULTS = "terminal/engine_defaults.c"
ENGINE = ("terminal/engine.c", ENGINE_DEFAULTS, "terminal/engine_parser.c",
          "terminal/engine_events.c", "terminal/engine_sync.c", "terminal/engine_snapshot.c",
          "terminal/engine_view_selection.c", "terminal/engine_paste.c")
SESSION_SUPPORT = ("terminal/session_view.c", "terminal/session_transfer.c", "terminal/render_sync.c",
                   "terminal/engine_adapter.c", *ENGINE)
COMMON = ("terminal/defaults.c", "terminal/st.c", *SESSION_SUPPORT,
          "workspace/wire.c", "workspace/workspace_socket.c")
CLIENT = (*COMMON, "workspace/workspace_client.c", "x11/x.c", "x11/url.c")
SERVER = (*COMMON, "workspace/workspace_service.c", "workspace/workspace_handoff.c",
          "workspace/workspace_restore.c", "workspace/workspace_launch.c", "workspace/workspace_upgrade.c",
          "workspace/browser_target.c")
PREVIEW = (*COMMON, "workspace/workspace_client.c", "gtk/app.c", "gtk/terminal.c",
           "gtk/browser_loader.c", "gtk/tabs.c", "gtk/windows.c", "gtk/window_state.c")
PREVIEW_PACKAGES = ("gtk+-3.0", "pangocairo", "fontconfig", "freetype2", "webkit2gtk-4.1")
TERMINAL_PACKAGES = ("gtk+-3.0", "pangocairo", "fontconfig", "freetype2", "gmodule-2.0")
SOURCES = tuple(dict.fromkeys((*CLIENT, *SERVER)))
CPPFLAGS = ("-I.", "-Isrc", "-Isrc/terminal", "-Isrc/x11", "-Isrc/workspace",
            f'-DVERSION="{VERSION}"', "-D_XOPEN_SOURCE=600")
PACKAGES = ("x11", "xft", "fontconfig", "freetype2")
PROBES = {"key_injector": ("x11", "xtst"), "placement_wm": ("x11",),
          "overlap_probe": ("x11",), "border_probe": ("x11",), "icon_probe": ("x11",), "latency_echo": ()}


def words(name, default=""):
    return shlex.split(os.environ.get(name, default))


def invoke(command):
    subprocess.run(command, cwd=ROOT, check=True)


def pkg_flags(option, packages=PACKAGES):
    if not packages:
        return []
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
    args = ([str(temporary) if part == "{output}" else part for part in command]
            if "{output}" in command else [*command, "-o", str(temporary)])
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


def generate_browser_resources():
    """Compile native browser chrome; web applications remain ordinary runtime files."""
    resources = {name: (ROOT / "src/gtk" / source).read_bytes() for name, source in
                 (("error_html", "browser-error.html"), ("find_script", "browser-find.js"),
                  ("local_script", "browser-local.js"))}
    if resources["error_html"].count(b"{{message}}") != 1:
        raise ValueError("Browser error page requires exactly one message slot")
    if resources["local_script"].count(b"%") != 2 or resources["local_script"].count(b"%s") != 2:
        raise ValueError("Local document script requires exactly two format slots")
    content = "/* Generated native browser resources. */\n"
    for name, data in resources.items():
        chunks = ['"' + "".join(f"\\{byte:03o}" for byte in data[start:start + 24]) + '"'
                  for start in range(0, len(data), 24)]
        content += f"static const char browser_{name}[] =\n" + "\n".join(chunks) + ";\n"
    header = ROOT / "build/browser_resources.h"
    header.parent.mkdir(parents=True, exist_ok=True)
    if not header.exists() or header.read_text() != content:
        header.write_text(content)


class Builder:
    def __init__(self):
        self.cc = words("CC", "cc")
        self.cflags = compile_flags()
        self.libs = link_flags()

    def program(self, output, sources, *, variant="default", flags=(), libs=None, link_dependencies=()):
        objects = []
        for source in sources:
            obj = f"build/obj/{variant}/{Path(source).with_suffix('.o')}"
            step(obj, [*self.cc, *self.cflags, *flags, "-c", source], [source], depfile=True)
            objects.append(obj)
        step(output, [*self.cc, *objects, *(self.libs if libs is None else libs)],
             [*objects, *link_dependencies])

    def rust_artifact(self, package, binary=None, *, library=False):
        if package in ("worminal-daemon", "worminal-frontend"):
            self.target("build/libworminal_engine.a")
        invoke(["cargo", "build", "--locked", "--release", "--quiet", "-p", package,
                *(["--lib"] if library else ["--bin", binary] if binary else [])])
        artifact = f"lib{package.replace('-', '_')}.so" if library else binary or package
        return ROOT / "build/cargo/release" / artifact

    def rust_program(self, output, package, binary=None):
        source = self.rust_artifact(package, binary) if binary else self.rust_artifact(package)
        self.publish_rust(output, source)

    def rust_library(self, output, package):
        self.publish_rust(output, self.rust_artifact(package, library=True))

    def publish_rust(self, output, source):
        destination = ROOT / output
        if destination.exists() and destination.read_bytes() == source.read_bytes():
            return
        temporary = destination.with_name(destination.name + ".pending")
        try:
            shutil.copyfile(source, temporary)
            temporary.chmod(0o755)
            temporary.replace(destination)
            destination.with_suffix(".command.json").unlink(missing_ok=True)
        finally:
            temporary.unlink(missing_ok=True)

    def target(self, target):
        if target == "worminal-rust":
            self.target("build/" + RUST_BROWSER_MODULE)
            self.target("build/" + RUST_FRONTEND_MODULE)
            self.rust_program(target, "worminal-launcher", "worminal-rust-launcher")
        elif target == "build/" + RUST_FRONTEND_MODULE:
            self.rust_library(target, "worminal-frontend")
        elif target == "build/" + RUST_BROWSER_MODULE:
            self.rust_library(target, "worminal-browser")
        elif target == "build/libworminal_engine.a":
            objects = []
            for source in ENGINE:
                obj = f"build/obj/engine/{Path(source).with_suffix('.o')}"
                step(obj, [*self.cc, *CPPFLAGS, *words("CPPFLAGS"), *words("CFLAGS", "-O3"),
                           "-ffunction-sections", "-fdata-sections",
                           "-c", f"src/{source}"], [f"src/{source}"], depfile=True)
                objects.append(obj)
            step(target, [*words("AR", "ar"), "rcs", "{output}", *objects], objects)
        elif target == "build/engine_test":
            self.program(target, ["tests/engine.c", *(f"src/{s}" for s in ENGINE)],
                         variant="engine-test", flags=["-O1", "-g", "-fsanitize=address,undefined"],
                         libs=["-fsanitize=address,undefined", "-lm"])
        elif target == "build/browser_cpu.so":
            self.program(target, ["tests/browser_cpu.c"], variant="browser-cpu", flags=["-fPIC"],
                         libs=["-shared", "-lrt"])
        elif target == "build/browser.so":
            generate_browser_resources()
            old_flags = self.cflags
            self.cflags = [*CPPFLAGS, *words("CPPFLAGS"), *words("CFLAGS", "-O3"),
                           *pkg_flags("--cflags", PREVIEW_PACKAGES)]
            try:
                self.program(target, ["src/gtk/browser.c", "src/workspace/browser_target.c"],
                             variant="browser", flags=["-fPIC"],
                             libs=["-shared", *pkg_flags("--libs", PREVIEW_PACKAGES), *words("LDFLAGS")])
            finally:
                self.cflags = old_flags
        elif target in ("worminal-x11", "worminald-x11"):
            sources = CLIENT if target == "worminal-x11" else SERVER
            self.program(target, [f"src/{source}" for source in sources],
                         flags=['-DWORKSPACE_DAEMON="worminald-x11"'])
        elif target in ("worminal", "worminal-preview"):
            self.target("build/browser.so")
            self.target("build/frontend.so")
            self.rust_program(target, "worminal-launcher")
        elif target in ("worminald", "worminald-preview"):
            self.rust_program(target, "worminal-daemon")
        elif target in ("build/worminald-c-v2", "build/frontend.so", "build/preview_test"):
            if target == "build/preview_test":
                generate_browser_resources()
            flags = ["-DWORMINAL_PREVIEW_PROTOCOL"]
            if target == "build/frontend.so":
                flags += ["-fPIC", "-ffunction-sections", "-fdata-sections"]
            server = target == "build/worminald-c-v2"
            sources = SERVER if server else PREVIEW
            if target == "build/preview_test":
                sources = ()
                flags += ["-O1", "-g"]
            libs = ["-lm", "-lrt", "-lutil", *words("LDFLAGS")]
            if target == "build/preview_test":
                libs += pkg_flags("--libs", ("wayland-client",))
            if target == "build/frontend.so":
                libs += ["-shared", "-Wl,--gc-sections", "-Wl,--version-script=src/gtk/frontend.map"]
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
                              "src/workspace/workspace_client.c", "src/workspace/browser_target.c"],
                             variant="preview-service" if server else "preview-test" if not sources else "frontend",
                             flags=flags, libs=libs,
                             link_dependencies=["src/gtk/frontend.map"] if target == "build/frontend.so" else [])
            finally:
                self.cflags = old_flags
        elif target in ("build/browser_generations_test", "build/incompatible_browser.so",
                        "build/unresolved_browser.so"):
            packages = (*PREVIEW_PACKAGES, "gmodule-2.0")
            old_flags = self.cflags
            self.cflags = [*CPPFLAGS, *words("CPPFLAGS"), *words("CFLAGS", "-O2"),
                           *pkg_flags("--cflags", packages)]
            library = target.endswith(".so")
            try:
                source = "tests/unresolved_browser.c" if target.endswith("unresolved_browser.so") else \
                    "tests/incompatible_browser.c" if library else "tests/browser_generations.c"
                self.program(target, [source],
                             variant="browser-generations", flags=["-fPIC"] if library else [],
                             libs=[*(["-shared"] if library else []), *pkg_flags("--libs", packages)])
            finally:
                self.cflags = old_flags
        elif target == "build/render_probe":
            old_flags = self.cflags
            self.cflags = [*CPPFLAGS, *words("CPPFLAGS"), *words("CFLAGS", "-O3"),
                           *pkg_flags("--cflags", TERMINAL_PACKAGES)]
            try:
                self.program(target, ["tests/render_probe.c", "src/workspace/wire.c"], variant="render-probe",
                             flags=["-DWORMINAL_PREVIEW_PROTOCOL"],
                             libs=[*pkg_flags("--libs", TERMINAL_PACKAGES), *words("LDFLAGS")])
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
        elif target == "build/frame_schedule_test":
            self.program(target, ["tests/frame_schedule.c"], variant="frame-schedule", libs=[])
        elif target == "build/workspace_client_test":
            self.program(target, ["tests/workspace_client.c", "src/workspace/workspace_client.c"],
                         variant="workspace-client", libs=["-pthread"])
        elif target == "build/wire_reference":
            self.program(target, ["tests/wire_reference.c", "src/workspace/wire.c",
                                  "src/workspace/workspace_socket.c"],
                         variant="wire-reference", flags=["-DWORMINAL_PREVIEW_PROTOCOL"], libs=[])
        elif target == "build/recovery_reference":
            self.program(target, ["tests/recovery_reference.c", "src/workspace/workspace_restore.c",
                                  "src/workspace/wire.c"],
                         variant="recovery-reference", flags=["-DWORMINAL_PREVIEW_PROTOCOL"], libs=[])
        elif target == "build/browser_target_reference":
            self.program(target, ["tests/browser_target_reference.c", "src/workspace/browser_target.c"],
                         variant="browser-target-reference", libs=[])
        elif target == "build/view_state_test":
            # view_state.c includes x.c to exercise internal state; compile that translation unit once.
            sources = ["tests/view_state.c", "src/terminal/defaults.c", f"src/{ENGINE_DEFAULTS}",
                       "src/x11/url.c", "src/workspace/wire.c",
                       "src/workspace/workspace_socket.c", "src/workspace/workspace_client.c"]
            self.program(target, sources, variant="view", flags=["-ffunction-sections", "-fdata-sections"],
                         libs=["-Wl,--gc-sections", *self.libs, *pkg_flags("--libs", ("xtst",))])
        elif target in ("build/scrollback_test", "build/url_test"):
            scrollback = target == "build/scrollback_test"
            support = ("terminal/defaults.c", *SESSION_SUPPORT)
            sources = (["tests/scrollback.c", *(f"src/{source}" for source in support)] if scrollback
                       else ["tests/url.c", "src/x11/url.c"])
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
            "src/gtk/tabs.c", "src/gtk/windows.c", "src/gtk/window_state.c",
            "src/gtk/windows_forward.c", "src/gtk/launcher.c", "--",
            *flags, *pkg_flags("--cflags", PREVIEW_PACKAGES)])


def clean():
    # Own only build outputs: preserve .checks logs and captures used to diagnose failures.
    if (ROOT / "build").exists():
        shutil.rmtree(ROOT / "build")
    for name in ("worminal", "worminald", "worminal-x11", "worminald-x11", "worminal-preview", "worminald-preview",
                 "worminal-rust"):
        for suffix in ("", ".command.json", ".pending"):
            (ROOT / (name + suffix)).unlink(missing_ok=True)
    # Pre-Python builds left these objects outside the owned build directory.
    for name in ("st.o", "x.o"):
        (ROOT / name).unlink(missing_ok=True)


def installation_files(prefix=None):
    prefix = Path(prefix if prefix is not None else os.environ.get("PREFIX", Path.home() / ".local"))
    manprefix = Path(os.environ.get("MANPREFIX", str(prefix / "share/man")))
    if not prefix.is_absolute() or not manprefix.is_absolute():
        raise ValueError("PREFIX and MANPREFIX must be absolute paths")
    staging = Path(os.environ.get("DESTDIR", "/"))
    # Publish modules before a launcher that may immediately need them.
    files = [("build/browser.so", prefix / "lib/worminal" / BROWSER_MODULE, 0o755),
             ("build/frontend.so", prefix / "lib/worminal/frontend.so", 0o755)]
    files += [(name, prefix / "bin" / name, 0o755)
              for name in ("worminal", "worminald", "worminal-x11", "worminald-x11")]
    files += [("docs/man/worminal.1", manprefix / "man1/worminal.1", 0o644),
              ("assets/worminal.desktop", prefix / "share/applications/worminal.desktop", 0o644),
              ("assets/worminal.svg", prefix / "share/icons/hicolor/scalable/apps/worminal.svg", 0o644)]
    return [(ROOT / source, staging / str(destination).lstrip("/"), mode) for source, destination, mode in files]


def install(prefix=None, *, skip_build=False):
    files = installation_files(prefix)
    # Privileged publication must retain the regular user's compiled appearance.
    if not skip_build:
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
