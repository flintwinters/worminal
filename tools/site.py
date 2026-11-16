"""Build the public GitHub Pages site without a framework or network dependencies."""
from pathlib import Path
import shutil
from xml.sax.saxutils import escape

from tools.paths import BUILD, ROOT

URL = "https://flintwinters.github.io/worminal/"
SOURCE = ROOT / "website"
OUTPUT = BUILD / "site"


def build():
    if OUTPUT.exists():
        shutil.rmtree(OUTPUT)
    OUTPUT.mkdir(parents=True, exist_ok=True)
    for source in SOURCE.iterdir():
        if source.is_file():
            shutil.copyfile(source, OUTPUT / source.name)
    shutil.copyfile(ROOT / "assets/worminal.svg", OUTPUT / "worminal.svg")
    for name in ("index.html", "404.html"):
        path = OUTPUT / name
        path.write_text(path.read_text().replace("{{SITE_URL}}", URL))
    (OUTPUT / ".nojekyll").touch()
    (OUTPUT / "sitemap.xml").write_text(
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n'
        f'  <url><loc>{escape(URL)}</loc></url>\n</urlset>\n')
    print(f"Website: {OUTPUT}")
    return OUTPUT


def render_social_card():
    """Author the checked-in sharing image; normal builds do not need Pillow."""
    from PIL import Image, ImageDraw, ImageFont
    image = Image.new("RGB", (1200, 630), "#f5f1e9")
    draw = ImageDraw.Draw(image)
    fonts = Path("/usr/share/fonts/truetype/dejavu")
    face = lambda size: ImageFont.truetype(str(fonts / "DejaVuSans.ttf"), size)
    draw.rounded_rectangle((60, 55, 1140, 575), radius=24, fill="#202521")
    draw.text((105, 92), "~  Worminal", font=face(42), fill="#ffc0d4")
    draw.text((105, 205), "Close the window.", font=face(66), fill="#f5f1e9")
    draw.text((105, 290), "Keep the work.", font=face(66), fill="#ffc0d4")
    draw.text((108, 442), "Persistent shells. Remote machines. Web tools.", font=face(29), fill="#d0d5cf")
    image.save(SOURCE / "social.png", optimize=True)

