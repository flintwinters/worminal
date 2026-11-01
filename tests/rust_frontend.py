"""Compare Rust painting with the actual C frontend on private X11 and native Wayland."""
import json
from pathlib import Path
import subprocess

from PIL import Image, ImageChops
from frontend_perf import read_until
from preview import wayland_display
from visible_latency import stop
from x11 import isolated_display

ROOT = Path(__file__).resolve().parent.parent


def check(backend):
    prefix = ROOT / f".checks/rust-paint-{backend}"
    context = isolated_display if backend == "x11" else wayland_display
    with context() as env, prefix.with_suffix(".log").open("wb") as log:
        env.update(GDK_BACKEND=backend, WORMINAL_RENDER_FIXTURE="1", WORMINAL_RENDER_STYLES="1")
        process = subprocess.Popen([str(ROOT / "build/render_probe"), str(ROOT / "build/frontend.so"),
                                    str(prefix), "1"], env=env, stdin=subprocess.PIPE,
                                   stdout=subprocess.PIPE, stderr=log)
        try:
            cases = json.loads(read_until(process.stdout, b"\n", timeout=20))
            process.communicate(input=b"\n", timeout=5)
            if process.returncode:
                raise AssertionError(f"C render fixture failed; see {log.name}")
        finally:
            stop(process)
        for case in cases:
            source = Path(f"{prefix}-{case['case']}.png")
            output = source.with_name(source.stem + "-rust.png")
            with Image.open(source) as image:
                width, height = image.size
                subprocess.run([str(ROOT / "build/cargo/release/worminal-render-check"),
                                str(source.with_suffix(".wire")), str(output), str(case['cols']),
                                str(case['rows']), str(width), str(height),
                                "clipped" if case['case'] == 'clipped_ascii' else "full",
                                str(case['mode']), str(case['cursor']), str(case['focused']),
                                str(case['selection']), str(case['fractional']), str(case['styled'])],
                               env=env, stdout=log, stderr=log, check=True, timeout=20)
                with Image.open(output) as rendered:
                    if (image.size != rendered.size
                            or image.convert("RGBA").tobytes() != rendered.convert("RGBA").tobytes()):
                        ImageChops.difference(image.convert("RGBA"), rendered.convert("RGBA")).save(
                            source.with_name(source.stem + "-difference.png"))
                        raise AssertionError(f"Rust pixels differ from C: {backend} {case['case']}; see {prefix}")
    print(f"Rust painter {backend}: {len(cases)} exact C pixel cases, "
          "including cursors, selection and fractional grids.")


def main():
    binary = ROOT / "build/cargo/release/worminal-render-check"
    symbols = subprocess.check_output(["nm", "--defined-only", str(binary)], text=True)
    names = {line.split()[-1] for line in symbols.splitlines() if line.split()}
    if names & {"worminal_engine_feed", "te_twrite", "te_tnew"}:
        raise AssertionError("Imported Rust view retains unused parser/owner code")
    dependencies = subprocess.check_output(["ldd", str(binary)], text=True)
    if "libwebkit" in dependencies:
        raise AssertionError("Terminal frontend links WebKit before browser loading")
    for backend in ("x11", "wayland"):
        check(backend)


if __name__ == "__main__":
    main()
