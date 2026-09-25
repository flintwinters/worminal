"""Alacritty import and palette compilation checks."""

from pathlib import Path
import os
import unittest
from unittest.mock import patch

from tools.theme import find_config, read_config, theme_header


FIXTURES = Path(__file__).resolve().parent / "fixtures" / "alacritty"


class ThemeTest(unittest.TestCase):
    def test_default_alacritty_palette(self):
        header = theme_header({})
        self.assertIn('[0] = "#181818"', header)
        self.assertIn('[1] = "#ac4242"', header)
        self.assertIn('[15] = "#f8f8f8"', header)
        self.assertIn('[258] = "#d8d8d8"', header)
        self.assertIn('[259] = "#181818"', header)
        self.assertIn('[261] = "#0f0f0f"', header)
        self.assertIn('[269] = "#8e8e8e"', header)
        self.assertIn("theme_bold_bright = 0", header)
        self.assertIn("theme_font_offset_y = 0", header)
        self.assertIn("theme_glyph_offset_y = 0", header)
        self.assertIn("theme_font_size = 0.0", header)
        self.assertIn("theme_font_family = NULL", header)
        self.assertIn("theme_history_size = 10000", header)
        self.assertIn("theme_scroll_multiplier = 3", header)

    def test_imports_override_by_color_key(self):
        header = theme_header(read_config(FIXTURES / "main.toml"))
        for entry in (
            '[1] = "#abcdef"', '[12] = "#aabbcc"',
            '[196] = "#123456"', '[256] = "#eeeeee"',
            '[257] = "#454545"', '[258] = "#eeeeee"',
            '[259] = "#222222"', '[260] = "#fedcba"',
            '[261] = "#0f0f0f"', '[263] = "#556677"',
            '[264] = "#a17e4d"', '[269] = "#808080"',
            "theme_bold_bright = 1",
            "theme_font_offset_y = -3",
            "theme_font_size = 9.0",
            'theme_font_family = "Iosevka Term"',
            "theme_history_size = 64",
            "theme_scroll_multiplier = 5",
        ):
            self.assertIn(entry, header)

    def test_hex_prefix_and_derived_dim_colors(self):
        header = theme_header({"colors": {
            "normal": {"red": "0x123456"},
            "primary": {"foreground": "0xabcdef"},
        }})
        self.assertIn('[1] = "#123456"', header)
        self.assertIn('[262] = "#0b2238"', header)
        self.assertIn('[269] = "#70879d"', header)

    def test_font_family_is_quoted_for_generated_c(self):
        header = theme_header({"font": {"normal": {"family": 'Mono: "Book"'}}})
        self.assertIn('theme_font_family = "Mono: \\"Book\\""', header)

    def test_legacy_yaml_is_discovered_and_compiled(self):
        with patch.dict(os.environ, {"XDG_CONFIG_HOME": str(FIXTURES.parent)}, clear=False):
            path = find_config()
        self.assertEqual(path, FIXTURES / "alacritty.yml")
        header = theme_header(read_config(path))
        for entry in (
            'theme_font_family = "Input Mono Condensed"',
            "theme_font_size = 6.0",
            "theme_font_offset_y = -7",
            "theme_glyph_offset_y = -4",
            "theme_bold_bright = 1",
            '[1] = "#abcdef"',
            '[258] = "#f9e7c4"',
            '[259] = "#1d1f21"',
        ):
            self.assertIn(entry, header)

    def test_legacy_bold_setting_at_root(self):
        self.assertIn("theme_bold_bright = 1",
                      theme_header({"draw_bold_text_with_bright_colors": True}))

    def test_invalid_color_and_import_cycle_fail_build(self):
        with self.assertRaisesRegex(ValueError, "#RRGGBB"):
            theme_header({"colors": {"normal": {"red": "red; }"}}})
        with self.assertRaisesRegex(ValueError, "import cycle"):
            read_config(FIXTURES / "cycle_a.toml")

    def test_invalid_line_offset_and_history_fail_build(self):
        with self.assertRaisesRegex(ValueError, "font.offset.y"):
            theme_header({"font": {"offset": {"y": 1.5}}})
        with self.assertRaisesRegex(ValueError, "font.glyph_offset.y"):
            theme_header({"font": {"glyph_offset": {"y": 1.5}}})
        with self.assertRaisesRegex(ValueError, "font.size"):
            theme_header({"font": {"size": -1}})
        with self.assertRaisesRegex(ValueError, "font.normal.family"):
            theme_header({"font": {"normal": {"family": ""}}})
        with self.assertRaisesRegex(ValueError, "font.normal.family"):
            theme_header({"font": {"normal": {"family": 42}}})
        with self.assertRaisesRegex(ValueError, "scrolling.history"):
            theme_header({"scrolling": {"history": 100001}})
