"""Differential checks against the immutable pre-extraction C parser, plus both engine boundaries."""
import difflib
import json
import os
import statistics
import subprocess

from tools import build

BASELINE = "b48c871"


def check():
    build.generate_theme()
    builder = build.Builder()
    builder.target("build/libworminal_engine.a")
    symbols = subprocess.check_output(["nm", "-u", "build/libworminal_engine.a"],
                                      cwd=build.ROOT, text=True)
    forbidden = {"read", "write", "close", "open", "fopen", "fwrite", "fprintf", "vfprintf", "putc",
                 "syslog", "clock_gettime", "gettimeofday", "fork", "vfork", "execve", "posix_spawn",
                 "waitpid", "waitid", "kill", "signal", "sigaction", "ioctl", "poll", "ppoll", "select",
                 "pselect", "socket", "connect", "accept", "bind", "listen", "pipe", "pipe2", "openpty"}
    gui = ("gtk_", "gdk_", "pango_", "cairo_", "webkit_", "tsession", "tty", "xset", "xget", "xdraw",
           "xclip", "xbell")
    imports = {parts[-1] for line in symbols.splitlines() if len(parts := line.split()) == 2 and parts[0] == "U"}
    violations = {name for name in imports if name in forbidden or name.startswith(gui)}
    if violations:
        raise RuntimeError(f"Terminal engine imports application/OS operations: {sorted(violations)}")
    builder.target("build/engine_test")
    builder.target("build/scrollback_test")
    for binary in ("build/engine_test", "build/scrollback_test"):
        environment = {**os.environ, "ASAN_OPTIONS": f"detect_leaks={int(binary == 'build/engine_test')}"}
        subprocess.run([str(build.ROOT / binary)], cwd=build.ROOT, env=environment, check=True)
    from tools.reference import module
    baseline = module().ROOT / "oracle-baseline"
    original = ("st.c", "st.h", "st_state.h", "win.h", "defaults.c", "session_view.c", "session_view.h",
                "session_transfer.c", "session_transfer.h", "render_sync.c", "render_sync.h")
    for name in original:
        target = baseline / "src/terminal" / name
        target.parent.mkdir(parents=True, exist_ok=True)
        content = subprocess.check_output(["git", "show", f"{BASELINE}:src/terminal/{name}"], cwd=build.ROOT)
        if not target.exists() or target.read_bytes() != content:
            target.write_bytes(content)
    old = baseline.relative_to(build.ROOT).as_posix()
    support = ("session_view.c", "session_transfer.c", "render_sync.c", "defaults.c")
    old_flags = builder.cflags
    flags = ["-ffunction-sections", "-fdata-sections"]
    libs = ["-Wl,--gc-sections", "-lm"]
    try:
        builder.cflags = [f"-I{old}/src/terminal", *old_flags]
        builder.program("build/engine_oracle_baseline", ["tests/engine_oracle.c",
                        *(f"{old}/src/terminal/{name}" for name in support)],
                        variant="engine-oracle-baseline", flags=[*flags,
                        f'-DWORMINAL_ORACLE_SOURCE="{old}/src/terminal/st.c"'], libs=libs)
    finally:
        builder.cflags = old_flags
    builder.program("build/engine_oracle", ["tests/engine_oracle.c", "src/terminal/defaults.c",
                    *(f"src/{source}" for source in build.SESSION_SUPPORT)],
                    variant="engine-oracle", flags=flags, libs=libs)
    outputs = []
    for name in ("engine_oracle_baseline", "engine_oracle"):
        output = subprocess.check_output([str(build.ROOT / "build" / name)], cwd=build.ROOT)
        (build.ROOT / ".checks" / f"{name}.txt").write_bytes(output)
        outputs.append(output.decode().splitlines())
    if outputs[0] != outputs[1]:
        print("Parser behavior differs from the C baseline:")
        print("\n".join(list(difflib.unified_diff(*outputs, fromfile="baseline", tofile="engine"))[:30]))
        return 1
    samples = {name: [] for name in ("engine_oracle_baseline", "engine_oracle")}
    for trial in range(3):
        for name in list(samples)[::1 if trial % 2 == 0 else -1]:
            result = subprocess.check_output([str(build.ROOT / "build" / name), "--bench"],
                                             cwd=build.ROOT, timeout=30)
            samples[name].append(json.loads(result))
    assert len({sample["checksum"] for group in samples.values() for sample in group}) == 1
    report = {name: {"samples": values,
                    "cpu_ns": statistics.median(sample["cpu_ns"] for sample in values),
                    "rss_kib": statistics.median(sample["rss_kib"] for sample in values)}
              for name, values in samples.items()}
    (build.ROOT / ".checks/engine-resources.json").write_text(json.dumps(report, indent=2) + "\n")
    before, after = report.values()
    print(f"Parser CPU {after['cpu_ns'] / before['cpu_ns']:.2f}x baseline; "
          f"peak RSS {before['rss_kib']} -> {after['rss_kib']} KiB", flush=True)
    build.invoke(["cargo", "test", "--locked", "--quiet", "-p", "worminal-engine-sys",
                  "-p", "worminal-engine"])
    print("Engine: baseline equivalence, sanitizer boundaries, legacy adapters, Rust wrapper OK")
    return 0
