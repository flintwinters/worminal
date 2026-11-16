"""Build the public GitHub Pages site without a framework or network dependencies."""
from pathlib import Path
import hashlib
import re
import shutil
from xml.sax.saxutils import escape

from tools.paths import BUILD, ROOT

URL = "https://flintwinters.github.io/worminal/"
SOURCE = ROOT / "website"
OUTPUT = BUILD / "site"


def palette():
    """The page, browser theme and sharing image use the stylesheet's color tokens."""
    root = (SOURCE / "style.css").read_text().split('}', 1)[0]
    return dict(re.findall(r'--([\w-]+):\s*(#[0-9a-fA-F]{6})\s*;', root))


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
        content = path.read_text().replace("{{SITE_URL}}", URL)
        content = content.replace("{{THEME_COLOR}}", palette()['background'])
        # GitHub Pages caches assets: new colors must not reuse the previous CSS/card URL.
        for asset in ('style.css', 'social.png'):
            digest = hashlib.sha256((OUTPUT / asset).read_bytes()).hexdigest()[:12]
            content = content.replace(f'{asset}"', f'{asset}?v={digest}"')
        path.write_text(content)
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
    colors = palette()
    image = Image.new("RGB", (1200, 630), colors['background'])
    draw = ImageDraw.Draw(image)
    fonts = Path("/usr/share/fonts/truetype/dejavu")
    face = lambda size: ImageFont.truetype(str(fonts / "DejaVuSans.ttf"), size)
    draw.rounded_rectangle((60, 55, 1140, 575), radius=24, fill=colors['panel'])
    draw.text((105, 92), "~  Worminal", font=face(42), fill=colors['pink'])
    draw.text((105, 205), "Close the window.", font=face(66), fill=colors['text'])
    draw.text((105, 290), "Keep the work.", font=face(66), fill=colors['lavender'])
    position = 108
    font = face(29)
    for text, color in (("Persistent shells.  ", "green"), ("Remote machines.  ", "blue"),
                        ("Web tools.", "peach")):
        draw.text((position, 442), text, font=font, fill=colors[color])
        position += draw.textlength(text, font=font)
    image.save(SOURCE / "social.png", optimize=True)

