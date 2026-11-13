"""Runtime configuration as seen by tools: the `worminald config` resolver, explorer font and C reference header.

Resolution rules themselves are unit-tested in src/config; these checks cover the CLI contract and adapters.
"""

import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from tools import theme
from tools.build import Builder

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "alacritty"


def config_json(*extra, **environment):
    daemon = Builder().rust_artifact("worminal-daemon")
    env = {**os.environ, "HOME": str(FIXTURES), "XDG_CONFIG_HOME": str(FIXTURES / "none"), **environment}
    return subprocess.run([str(daemon), "config", *extra], env=env, capture_output=True, text=True)


class ThemeTest(unittest.TestCase):
    def test_worminal_yaml_layers_over_alacritty_config(self):
        result = config_json("--json", WORMINAL_ALACRITTY_CONFIG=str(FIXTURES / "main.toml"),
                             WORMINAL_CONFIG=str(FIXTURES / "worminal.yaml"))
        self.assertEqual(result.returncode, 0, result.stderr)
        values = json.loads(result.stdout)
        self.assertEqual((values["font_family"], values["font_size"], values["history_size"]),
                         ("Iosevka Term", 11.0, 64))
        self.assertTrue(values["update_notifications"])
        self.assertEqual(len(values["colors"]), 270)

    def test_invalid_configuration_is_reported_with_nonzero_status(self):
        with self.subTest("missing explicit file"):
            result = config_json(WORMINAL_CONFIG=str(FIXTURES / "absent.yaml"))
            self.assertEqual(result.returncode, 1)
            self.assertIn("does not exist", result.stderr)

    def test_the_executable_teaches_how_to_configure(self):
        """An agent needs no docs: --help, --example and --paths say what to edit, and validation names typos."""
        self.assertIn("worminald config --paths", config_json("--help").stdout)
        example = config_json("--example", WORMINAL_CONFIG="").stdout
        with tempfile.TemporaryDirectory(prefix="config-", dir=Path(__file__).resolve().parents[1] / "build") as root:
            written = Path(root) / "worminal.yaml"
            written.write_text(example + "font:\n  sizee: 12\n")
            result = config_json(WORMINAL_CONFIG=str(written))
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("font.sizee", result.stdout)
            paths = config_json("--paths", XDG_CONFIG_HOME=root).stdout
            self.assertIn(f"worminal_default: {root}/worminal/worminal.yaml", paths)

    def test_defaults_ignore_every_file(self):
        result = config_json("--json", "--defaults", WORMINAL_ALACRITTY_CONFIG=str(FIXTURES / "main.toml"))
        self.assertEqual(json.loads(result.stdout)["font_family"], "Atkinson Hyperlegible Mono")

    def test_explorer_line_height_uses_terminal_metrics_and_offset(self):
        values = {"font_family": "monospace", "font_size": 14.0, "font_offset_y": -7}
        with patch.object(theme, "settings", return_value=values), \
                patch.object(theme, "font_height", return_value=25) as metrics:
            self.assertEqual(theme.explorer_font(), {"family": "monospace", "size": 14.0, "lineHeight": 18})
            metrics.assert_called_once_with("monospace", 14.0)
            values["font_offset_y"] = -100
            self.assertEqual(theme.explorer_font()["lineHeight"], 1)
        self.assertGreater(theme.font_height("monospace", 14), 14)

    def test_legacy_header_quotes_family_and_keeps_unset_slots_out(self):
        values = {"font_family": 'Aé "Font" \\ family', "font_size": 13.5, "font_offset_y": -3,
                  "glyph_offset_y": 0, "history_size": 64, "scroll_multiplier": 5, "bold_bright": True,
                  "colors": ["#000000", None, "#ffffff"]}
        header = theme.legacy_header(values)
        self.assertIn('theme_font_family = "Aé \\"Font\\" \\\\ family";', header)
        self.assertIn("theme_font_size = 13.5;", header)
        self.assertIn("theme_bold_bright = 1;", header)
        self.assertIn('[2] = "#ffffff",', header)
        self.assertNotIn("[1] =", header)


if __name__ == "__main__":
    unittest.main()
