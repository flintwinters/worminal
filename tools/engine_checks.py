"""Sanitized public C engine boundaries, ownership separation and Rust wrappers."""
import os
import subprocess

from tools import build


def check():
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
    subprocess.run([str(build.ROOT / "build/engine_test")], cwd=build.ROOT,
                   env={**os.environ, "ASAN_OPTIONS": "detect_leaks=1"}, check=True)
    build.invoke(["cargo", "test", "--locked", "--quiet", "-p", "worminal-engine-sys",
                  "-p", "worminal-engine"])
    print("Engine: sanitizer boundaries and Rust wrapper OK")
    return 0
