"""Native build, analysis, and installation; invoked through manage.py."""

import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess

from tools.theme import generate_theme
from tools.paths import runtime_binary


ROOT = Path(__file__).resolve().parent.parent
VERSION = next(line.split('"')[1] for line in (ROOT / "src/version.h").read_text().splitlines()
               if line.startswith("#define WORMINAL_VERSION "))
RUST_BROWSER_MODULE = next(line.split('"')[1]
                          for line in (ROOT / "src/gtk/rust_browser_module.h").read_text().splitlines()
                          if line.startswith("#define WORMINAL_RUST_BROWSER_FILE "))
RUST_FRONTEND_MODULE = next(line.split('"')[1]
                           for line in (ROOT / "src/gtk/rust_frontend_module.h").read_text().splitlines()
                           if line.startswith("#define WORMINAL_RUST_FRONTEND_FILE "))
RUST_FRONTEND_SYMBOL = next(line.split('"')[1]
                           for line in (ROOT / "src/gtk/rust_frontend_module.h").read_text().splitlines()
                           if line.startswith("#define WORMINAL_RUST_FRONTEND_SYMBOL "))
ENGINE_DEFAULTS = "terminal/engine_defaults.c"
ENGINE = ("terminal/engine.c", ENGINE_DEFAULTS, "terminal/engine_parser.c",
          "terminal/engine_events.c", "terminal/engine_sync.c", "terminal/engine_snapshot.c",
          "terminal/engine_view_selection.c", "terminal/engine_paste.c")
CPPFLAGS = ("-I.", "-Isrc", "-Isrc/terminal", "-Isrc/workspace",
            f'-DVERSION="{VERSION}"', "-D_XOPEN_SOURCE=600")
PACKAGES = ()
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
        destination = runtime_binary(output, prefix=ROOT / "build") if "/" not in output else ROOT / output
        destination.parent.mkdir(parents=True, exist_ok=True)
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
            self.target("build/lib/worminal/" + RUST_BROWSER_MODULE)
            self.target("build/lib/worminal/" + RUST_FRONTEND_MODULE)
            self.rust_program(target, "worminal-launcher", "worminal-rust-launcher")
        elif target == "build/lib/worminal/" + RUST_FRONTEND_MODULE:
            self.rust_library(target, "worminal-frontend")
        elif target == "build/lib/worminal/" + RUST_BROWSER_MODULE:
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
        elif target == "build/gui-peer.so":
            self.program(target, ["tests/gui_peer.c"], variant="gui-peer", flags=["-fPIC"], libs=["-shared", "-ldl"])
        elif target in ("worminal", "worminal-preview"):
            self.target("build/lib/worminal/" + RUST_BROWSER_MODULE)
            self.target("build/lib/worminal/" + RUST_FRONTEND_MODULE)
            self.rust_program(target, "worminal-launcher")
        elif target in ("worminald", "worminald-preview"):
            self.rust_program(target, "worminal-daemon")
        elif target.removeprefix("build/") in PROBES and target.startswith("build/"):
            name = target.removeprefix("build/")
            packages = PROBES[name]
            step(target, [*self.cc, "-O2", *pkg_flags("--cflags", packages), f"tests/{name}.c",
                          *pkg_flags("--libs", packages)], [f"tests/{name}.c"])
        elif target == "build/scrollback_test":
            support = ("terminal/defaults.c", *__getattr__("SESSION_SUPPORT"))
            self.program(target, ["tests/scrollback.c", *(f"src/{source}" for source in support)],
                         variant="scrollback_test", flags=["-O1", "-g", "-fsanitize=address,undefined",
                         "-ffunction-sections", "-fdata-sections"],
                         libs=["-fsanitize=address,undefined", "-Wl,--gc-sections", "-lm"])
        else:
            from tools.reference import target as historical_target
            historical_target(target)


def __getattr__(name):
    # Test fixtures inherit their historical build decisions from Git; shipping builds never enter here.
    if name in ("SESSION_SUPPORT", "PREVIEW_PACKAGES", "TERMINAL_PACKAGES", "BROWSER_MODULE",
                "SERVER", "CLIENT", "COMMON", "PREVIEW", "SOURCES"):
        from tools.reference import module
        historical = module()
        if name == "SESSION_SUPPORT":
            return (*[source for source in historical.SESSION_SUPPORT if source not in historical.ENGINE], *ENGINE)
        return getattr(historical, name)
    raise AttributeError(name)


def build_targets(*targets):
    generate_theme()
    builder = Builder()
    for target in dict.fromkeys(("worminal", "worminald", *targets)):
        builder.target(target)


def analyze():
    build_targets()
    tidy = [*words("CLANG_TIDY", "clang-tidy"), "-quiet", "--config-file=tools/clang-tidy.yaml"]
    invoke([*tidy, *(f"src/{source}" for source in ENGINE), "--", *compile_flags()])
    invoke(["cargo", "check", "--locked", "--workspace", "--all-targets"])


def clean():
    from tools import reference
    if ROOT == reference.ROOT:
        reference.cleanup()
    reference.remove_fixture_links(ROOT)
    # Preserve diagnostics and user-owned virtualenvs; compiled output and caches are disposable.
    directory = ROOT / 'build'
    if directory.exists():
        for child in directory.iterdir():
            if child.name in ('checks', 'venv'):
                continue
            if child.is_dir() and not child.is_symlink():
                shutil.rmtree(child)
            else:
                child.unlink()
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
    files = [("build/lib/worminal/" + RUST_BROWSER_MODULE, prefix / "lib/worminal" / RUST_BROWSER_MODULE, 0o755),
             ("build/lib/worminal/" + RUST_FRONTEND_MODULE, prefix / "lib/worminal" / RUST_FRONTEND_MODULE, 0o755)]
    files += [("build/bin/" + name, prefix / "bin" / name, 0o755)
              for name in ("worminal", "worminald")]
    files += [("docs/man/worminal.1", manprefix / "man1/worminal.1", 0o644),
              ("assets/worminal.desktop", prefix / "share/applications/worminal.desktop", 0o644),
              ("assets/worminal.svg", prefix / "share/icons/hicolor/scalable/apps/worminal.svg", 0o644)]
    return [(ROOT / source, staging / str(destination).lstrip("/"), mode) for source, destination, mode in files]


def retired_installation_files(prefix=None):
    # Retired filenames are cleanup ownership, never build or recovery targets. Profiles are user data.
    prefix = Path(prefix if prefix is not None else os.environ.get("PREFIX", Path.home() / ".local"))
    staging = Path(os.environ.get("DESTDIR", "/"))
    root = staging / str(prefix).lstrip("/")
    paths = [root / "bin" / name for name in ("worminal-x11", "worminald-x11")]
    modules = root / "lib/worminal"
    paths += [modules / "frontend.so", modules / "browser.so"]
    paths += [path for path in modules.glob("browser-v*.so")
              if path.name.removeprefix("browser-v").removesuffix(".so").isdigit()]
    return paths


def install(prefix=None, *, skip_build=False):
    files = installation_files(prefix)
    # Build before sudo so the result uses the user's theme.
    if not skip_build:
        build_targets()
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
    for path in retired_installation_files(prefix):
        path.unlink(missing_ok=True)
    print("Installed Rust binaries/modules, manual, desktop launcher, and icon; retired C artifacts removed.")


def uninstall(prefix=None):
    for _, destination, _ in installation_files(prefix):
        destination.unlink(missing_ok=True)
    print("Removed installed Worminal files.")
