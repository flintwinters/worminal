"""Public Pages contracts: crawlable content, valid links and consistent project URLs."""
from html.parser import HTMLParser
import json
import struct
import unittest
from urllib.parse import unquote, urljoin, urlsplit
from xml.etree import ElementTree

from tools import site
from tools.paths import ROOT


class Page(HTMLParser):
    def __init__(self, source):
        super().__init__()
        self.nodes, self.text, self.schemas = [], [], []
        self.schema = None
        self.ignore = None
        self.headings = []
        self.heading = None
        self.feed(source)

    def handle_starttag(self, tag, attrs):
        attributes = dict(attrs)
        self.nodes.append((tag, attributes))
        if tag == 'script' and attributes.get('type') == 'application/ld+json':
            self.schema = ''
        elif tag in ('script', 'style'):
            self.ignore = tag
        if tag in ('h1', 'h2', 'h3'):
            self.heading = [tag, '']

    def handle_data(self, data):
        if self.schema is not None:
            self.schema += data
        elif self.ignore is None:
            self.text.append(data)
            if self.heading is not None:
                self.heading[1] += data

    def handle_endtag(self, tag):
        if tag == 'script' and self.schema is not None:
            self.schemas.append(json.loads(self.schema))
            self.schema = None
        if tag == self.ignore:
            self.ignore = None
        if self.heading is not None and tag == self.heading[0]:
            self.headings.append(tuple(self.heading))
            self.heading = None


class SiteTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.output = site.build()
        cls.source = (cls.output / 'index.html').read_text()
        cls.page = Page(cls.source)

    def test_navigation_assets_and_repository_references_resolve(self):
        identifiers = [attrs['id'] for _, attrs in self.page.nodes if 'id' in attrs]
        self.assertEqual(len(identifiers), len(set(identifiers)), 'Duplicate fragment identifiers')
        for tag, attrs in self.page.nodes:
            if tag == 'img':
                self.assertIn('alt', attrs)
            for identifier in attrs.get('aria-labelledby', '').split():
                self.assertIn(identifier, identifiers)
            for field in ('href', 'src'):
                if not (value := attrs.get(field)):
                    continue
                target = urlsplit(urljoin(site.URL, value))
                if target.netloc == urlsplit(site.URL).netloc:
                    prefix = urlsplit(site.URL).path
                    self.assertTrue(target.path.startswith(prefix), value)
                    relative = unquote(target.path.removeprefix(prefix)) or 'index.html'
                    self.assertTrue((self.output / relative).is_file(), value)
                    if target.fragment:
                        self.assertIn(target.fragment, identifiers, value)
                elif target.netloc == 'github.com' and '/worminal/blob/main/' in target.path:
                    relative = unquote(target.path.split('/worminal/blob/main/', 1)[1])
                    self.assertTrue((ROOT / relative).is_file(), value)

    def test_search_and_sharing_metadata_agree_with_published_url(self):
        self.assertNotIn('{{', self.source)
        self.assertEqual(sum(tag == 'h1' for tag, _ in self.page.nodes), 1)
        self.assertTrue(any(tag == 'html' and attrs.get('lang') == 'en' for tag, attrs in self.page.nodes))
        metas = {a.get('name', a.get('property')): a.get('content') for t, a in self.page.nodes if t == 'meta'}
        self.assertIn('width=device-width', metas['viewport'])
        self.assertEqual(metas['theme-color'], site.palette()['background'])
        self.assertTrue(metas['description'])
        self.assertIn('Linux terminal', next(text for tag, text in self.page.headings if tag == 'h1'))
        self.assertNotIn('noindex', metas.get('robots', ''))
        canonical = [a['href'] for t, a in self.page.nodes if t == 'link' and a.get('rel') == 'canonical']
        self.assertEqual(canonical, [site.URL])
        self.assertEqual(metas['og:url'], site.URL)
        self.assertEqual(metas['og:image'], metas['twitter:image'])
        png = (self.output / 'social.png').read_bytes()
        self.assertEqual(png[:8], b'\x89PNG\r\n\x1a\n')
        self.assertEqual(struct.unpack('>II', png[16:24]), (1200, 630))
        sitemap = ElementTree.parse(self.output / 'sitemap.xml')
        urls = sitemap.findall('.//{http://www.sitemaps.org/schemas/sitemap/0.9}loc')
        self.assertEqual([item.text for item in urls], [site.URL])

    def test_software_schema_is_truthful_and_content_is_static(self):
        self.assertEqual(len(self.page.schemas), 1)
        schema = self.page.schemas[0]
        self.assertEqual(schema['@type'], 'SoftwareApplication')
        self.assertEqual(schema['operatingSystem'], 'Linux')
        self.assertEqual(schema['url'], site.URL)
        self.assertNotIn('aggregateRating', schema)
        self.assertNotIn('review', schema)
        self.assertFalse(any(t == 'script' and 'src' in a for t, a in self.page.nodes))
        self.assertFalse(any(t == 'link' and a.get('rel') == 'stylesheet' for t, a in self.page.nodes))
        self.assertTrue(any(t == 'table' and a.get('id') == 'capabilities' for t, a in self.page.nodes))
        self.assertIn('SSH', ''.join(self.page.text))
        self.assertLess(sum(p.stat().st_size for p in self.output.iterdir() if p.is_file()), 250_000)

    def test_product_copy_and_navigation_exist_without_javascript(self):
        topics = [attrs for tag, attrs in self.page.nodes if tag == 'section' and 'data-panel' in attrs]
        self.assertEqual(len(topics), 4)
        self.assertTrue(all('hidden' not in topic for topic in topics))
        navigation = [attrs for _, attrs in self.page.nodes if 'data-section' in attrs]
        self.assertTrue(navigation)
        self.assertTrue(all(attrs.get('href', '').startswith('#panel-') for attrs in navigation))
        text = ' '.join(''.join(self.page.text).split())
        for feature in ('Persistent terminal sessions', 'Local and SSH sessions', 'HTML tools',
                        'native API for terminal agents', 'Get Worminal'):
            self.assertIn(feature, text)

    def test_reading_text_has_strong_contrast_on_page_surfaces(self):
        def luminance(color):
            values = [int(color[index:index + 2], 16) / 255 for index in (1, 3, 5)]
            linear = [value / 12.92 if value <= .04045 else ((value + .055) / 1.055) ** 2.4
                      for value in values]
            return sum(value * weight for value, weight in zip(linear, (.2126, .7152, .0722)))

        colors = site.palette()
        for foreground in ('text', 'muted'):
            for background in ('background', 'panel', 'well', 'raised'):
                first, second = sorted((luminance(colors[foreground]), luminance(colors[background])))
                self.assertGreaterEqual((second + .05) / (first + .05), 7,
                                        f'{foreground} lacks reading contrast on {background}')

    def test_unknown_pages_do_not_index_or_break_under_nested_urls(self):
        page = Page((self.output / '404.html').read_text())
        self.assertIn(('meta', {'name': 'robots', 'content': 'noindex'}), page.nodes)
        for tag, attrs in page.nodes:
            if tag in ('a', 'link'):
                self.assertTrue(attrs['href'].startswith((site.URL, 'data:image/svg+xml,')))

    def test_build_excludes_stale_artifacts(self):
        stale = self.output / 'obsolete.html'
        stale.write_text('This page must not be published')
        site.build()
        self.assertFalse(stale.exists())
        self.assertFalse((self.output / 'style.css').exists())
        self.assertFalse((self.output / 'app.js').exists())


if __name__ == '__main__':
    unittest.main()
