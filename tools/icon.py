"""Generate the X11 window icon from the desktop SVG."""

from pathlib import Path
import subprocess


ROOT = Path(__file__).resolve().parent.parent
SIZE = 48


def generate_icon():
    from PIL import Image

    cache = ROOT / "build"
    cache.mkdir(exist_ok=True)
    png = cache / "icon.png"
    subprocess.run(
        ["inkscape", str(ROOT / "assets/worminal.svg"),
         f"--export-filename={png}", f"--export-width={SIZE}",
         f"--export-height={SIZE}"],
        check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    with Image.open(png) as image:
        pixels = list(image.convert("RGBA").getdata())
    values = [SIZE, SIZE] + [
        (a << 24) | (r << 16) | (g << 8) | b if a else 0
        for r, g, b, a in pixels
    ]
    rows = ["/* Generated from assets/worminal.svg by `python3 manage.py icon`. */",
            "static const unsigned long worminal_icon[] = {"]
    rows += ["\t" + ", ".join(f"0x{value:08x}UL" for value in values[i:i + 6]) + ","
             for i in range(0, len(values), 6)]
    rows.append("};")
    output = ROOT / "src/x11/icon.h"
    content = "\n".join(rows) + "\n"
    if not output.exists() or output.read_text() != content:
        output.write_text(content)
