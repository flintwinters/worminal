"""Alacritty import and palette compilation checks."""

from pathlib import Path
import unittest

from tools.theme import read_config, theme_header


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

    def test_invalid_color_and_import_cycle_fail_build(self):
        with self.assertRaisesRegex(ValueError, "#RRGGBB"):
            theme_header({"colors": {"normal": {"red": "red; }"}}})
        with self.assertRaisesRegex(ValueError, "import cycle"):
            read_config(FIXTURES / "cycle_a.toml")
