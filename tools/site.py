"""Build the public GitHub Pages site without a framework or network dependencies."""
from pathlib import Path
import hashlib
import re
import shutil
from xml.sax.saxutils import escape
from urllib.parse import quote

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
    for name in ('index.html', '404.html', 'social.png'):
        shutil.copyfile(SOURCE / name, OUTPUT / name)
    logo = (ROOT / 'assets/worminal.svg').read_text()
    replacements = {
        '{{SITE_URL}}': URL,
        '{{THEME_COLOR}}': palette()['background'],
        '{{SITE_CSS}}': (SOURCE / 'style.css').read_text(),
        '{{SITE_JS}}': (SOURCE / 'app.js').read_text(),
        '{{LOGO_SVG}}': logo,
        '{{FAVICON}}': 'data:image/svg+xml,' + quote(logo, safe=''),
    }
    for name in ("index.html", "404.html"):
        path = OUTPUT / name
        content = path.read_text()
        for key, value in replacements.items():
            content = content.replace(key, value)
        # The console is self-contained; only off-page sharing uses a separate image.
        digest = hashlib.sha256((OUTPUT / 'social.png').read_bytes()).hexdigest()[:12]
        content = content.replace('social.png"', f'social.png?v={digest}"')
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
    face = lambda size: ImageFont.truetype(str(fonts / "DejaVuSansMono.ttf"), size)
    draw.rectangle((0, 0, 1200, 42), fill='#32302f')
    draw.text((22, 9), 'WORMINAL / LINUX TERMINAL', font=face(20), fill=colors['text'])

    def strip(bounds, label, color):
        x, y, right, bottom = bounds
        draw.rectangle(bounds, fill=colors['panel'])
        draw.rectangle((x, y, right, y + 28), fill=colors[color])
        rgb = tuple(bytes.fromhex(colors[color][1:]))
        draw.line((x, y, right, y), fill=tuple(round(value * .88 + 255 * .12) for value in rgb))
        draw.line((x, y + 28, right, y + 28), fill=tuple(round(value * .8) for value in rgb))
        draw.text((x + 10, y + 3), label, font=face(16), fill=colors['background'])

    strip((16, 55, 782, 398), 'FEATURES', 'blue')
    draw.text((42, 110), 'Worminal', font=face(54), fill=colors['text'])
    draw.text((43, 203), 'Shells keep running', font=face(37), fill=colors['yellow'])
    draw.text((43, 256), 'when windows close.', font=face(37), fill=colors['text'])
    draw.text((43, 342), '$ worminal --attach', font=face(23), fill=colors['aqua'])
    strip((792, 55, 1184, 398), 'RUNNING PROGRAMS', 'green')
    for index, text in enumerate(('WINDOW CLOSE', '  shells keep running', 'LIVE UPDATE', '  programs keep running',
                                  'REBOOT', '  fresh shells')):
        draw.text((810, 109 + index * 41), text, font=face(19),
                  fill=colors['muted'] if index % 2 == 0 else colors['text'])
    strip((16, 410, 1184, 576), 'ALL FEATURES', 'purple')
    for index, text in enumerate(('Persistent sessions / local + SSH', 'Web tools / split panes / agent API')):
        draw.text((42, 461 + index * 45), text, font=face(25), fill=colors['text'])
    draw.text((18, 597), 'LINUX / WAYLAND + X11', font=face(17), fill=colors['orange'])
    image.save(SOURCE / "social.png", optimize=True)

