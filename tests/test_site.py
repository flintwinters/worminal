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
        self.feed(source)

    def handle_starttag(self, tag, attrs):
        attributes = dict(attrs)
        self.nodes.append((tag, attributes))
        if tag == 'script' and attributes.get('type') == 'application/ld+json':
            self.schema = ''

    def handle_data(self, data):
        if self.schema is not None:
            self.schema += data
        else:
            self.text.append(data)

    def handle_endtag(self, tag):
        if tag == 'script' and self.schema is not None:
            self.schemas.append(json.loads(self.schema))
            self.schema = None


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
        self.assertTrue(metas['description'])
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
        self.assertFalse(any(t == 'script' and a.get('type') != 'application/ld+json'
                             for t, a in self.page.nodes), 'Marketing content must work without JavaScript')
        self.assertIn('SSH', ''.join(self.page.text))
        self.assertLess(sum(p.stat().st_size for p in self.output.iterdir() if p.is_file()), 250_000)

    def test_unknown_pages_do_not_index_or_break_under_nested_urls(self):
        page = Page((self.output / '404.html').read_text())
        self.assertIn(('meta', {'name': 'robots', 'content': 'noindex'}), page.nodes)
        for tag, attrs in page.nodes:
            if tag in ('a', 'link'):
                self.assertTrue(attrs['href'].startswith(site.URL))

    def test_build_excludes_stale_artifacts(self):
        stale = self.output / 'obsolete.html'
        stale.write_text('This page must not be published')
        site.build()
        self.assertFalse(stale.exists())


if __name__ == '__main__':
    unittest.main()
