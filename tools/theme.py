"""Resolve Alacritty appearance and scrolling into a C header at build time."""

import copy
import json
import math
import os
from pathlib import Path
import re
import tomllib


ROOT = Path(__file__).resolve().parent.parent
OUTPUT = ROOT / ".checks" / "theme.h"
NAMES = ("black", "red", "green", "yellow", "blue", "magenta", "cyan", "white")
DEFAULTS = {
    "primary": {"foreground": "#d8d8d8", "background": "#181818"},
    "normal": dict(zip(NAMES, (
        "#181818", "#ac4242", "#90a959", "#f4bf75",
        "#6a9fb5", "#aa759f", "#75b5aa", "#d8d8d8",
    ))),
    "bright": dict(zip(NAMES, (
        "#6b6b6b", "#c55555", "#aac474", "#feca88",
        "#82b8c8", "#c28cb8", "#93d3c3", "#f8f8f8",
    ))),
}
HEX_COLOR = re.compile(r"(?:#|0x)[0-9a-fA-F]{6}\Z")
DIM_DEFAULTS = dict(zip(NAMES, (
    "#0f0f0f", "#712b2b", "#5f6f3a", "#a17e4d",
    "#456877", "#704d68", "#4d7770", "#8e8e8e",
)))


def find_config():
    explicit = os.environ.get("WORMINAL_ALACRITTY_CONFIG")
    if explicit:
        path = Path(explicit).expanduser()
        if not path.is_file():
            raise ValueError(f"Alacritty config does not exist: {path}")
        return path
    home = Path.home()
    xdg = Path(os.environ.get("XDG_CONFIG_HOME", home / ".config"))
    names = ("alacritty.toml", "alacritty.yml", "alacritty.yaml")
    candidates = tuple(
        directory / name
        for directory in (xdg / "alacritty", xdg, home / ".config" / "alacritty")
        for name in names
    ) + tuple(home / f".{name}" for name in names) + tuple(
        Path("/etc/alacritty") / name for name in names
    )
    return next((path for path in candidates if path.is_file()), None)


def merge(target, source):
    for key, value in source.items():
        if isinstance(value, dict) and isinstance(target.get(key), dict):
            merge(target[key], value)
        else:
            target[key] = copy.deepcopy(value)
    return target


def read_config(path, stack=()):
    path = Path(path).resolve()
    if path in stack:
        raise ValueError(f"Alacritty import cycle: {path}")
    if path.suffix.lower() in (".yml", ".yaml"):
        try:
            import yaml
        except ModuleNotFoundError as error:
            raise ValueError("Install PyYAML to build from Alacritty YAML config") from error
        with path.open("r", encoding="utf-8") as source:
            config = yaml.safe_load(source)
        if config is None:
            config = {}
    else:
        with path.open("rb") as source:
            config = tomllib.load(source)
    if not isinstance(config, dict):
        raise ValueError(f"Alacritty config must be a table: {path}")
    imports = config.get("general", {}).get("import", config.get("import", []))
    if not isinstance(imports, list) or not all(isinstance(item, str) for item in imports):
        raise ValueError(f"Alacritty imports must be a list of paths: {path}")
    result = {}
    for item in imports:
        imported = Path(item).expanduser()
        if not imported.is_absolute():
            imported = path.parent / imported
        if imported.is_file():
            merge(result, read_config(imported, (*stack, path)))
    return merge(result, config)


def color(value, name):
    if not isinstance(value, str) or not HEX_COLOR.fullmatch(value):
        raise ValueError(f"{name} must be a #RRGGBB or 0xRRGGBB color")
    return "#" + value[-6:].lower()


def dim_color(value):
    # Alacritty's display/color.rs scales each RGB component by 0.66 and truncates.
    return "#" + "".join(f"{int(int(value[index:index + 2], 16) * 0.66):02x}"
                         for index in (1, 3, 5))


def theme_header(config):
    font = config.get("font", {})
    if not isinstance(font, dict):
        raise ValueError("[font] must be a table")
    normal = font.get("normal", {})
    if not isinstance(normal, dict):
        raise ValueError("font.normal must be a table")
    family = normal.get("family")
    if family is not None and (not isinstance(family, str) or not family.strip() or
                               any(ord(char) < 32 for char in family)):
        raise ValueError("font.normal.family must be a nonempty font family")
    offset = font.get("offset", {})
    if not isinstance(offset, dict):
        raise ValueError("font.offset must be a table")
    line_offset = offset.get("y", 0)
    if type(line_offset) is not int or not -(2**31) <= line_offset < 2**31:
        raise ValueError("font.offset.y must fit a signed 32-bit integer")
    glyph_offset = font.get("glyph_offset", {})
    if not isinstance(glyph_offset, dict):
        raise ValueError("font.glyph_offset must be a table")
    glyph_offset_y = glyph_offset.get("y", 0)
    if type(glyph_offset_y) is not int or not -(2**31) <= glyph_offset_y < 2**31:
        raise ValueError("font.glyph_offset.y must fit a signed 32-bit integer")
    font_size = font.get("size", 0)
    if type(font_size) not in (int, float) or not math.isfinite(font_size) or \
            ("size" in font and font_size <= 0):
        raise ValueError("font.size must be a positive number")
    scrolling = config.get("scrolling", {})
    if not isinstance(scrolling, dict):
        raise ValueError("[scrolling] must be a table")
    history = scrolling.get("history", 10000)
    multiplier = scrolling.get("multiplier", 3)
    if type(history) is not int or not 0 <= history <= 100000:
        raise ValueError("scrolling.history must be an integer from 0 to 100000")
    if type(multiplier) is not int or not 1 <= multiplier < 2**31:
        raise ValueError("scrolling.multiplier must be a positive integer")

    colors = config.get("colors", {})
    if not isinstance(colors, dict):
        raise ValueError("[colors] must be a table")
    tables = {}
    for group in ("primary", "normal", "bright", "dim", "cursor"):
        table = colors.get(group, {})
        if not isinstance(table, dict):
            raise ValueError(f"[colors.{group}] must be a table")
        tables[group] = table

    palette = {}
    for group, offset in (("normal", 0), ("bright", 8)):
        for index, name in enumerate(NAMES):
            value = tables[group].get(name, DEFAULTS[group][name])
            palette[offset + index] = color(value, f"colors.{group}.{name}")
    indexed = colors.get("indexed_colors", [])
    if not isinstance(indexed, list):
        raise ValueError("colors.indexed_colors must be a list")
    for entry in indexed:
        if not isinstance(entry, dict) or type(entry.get("index")) is not int:
            raise ValueError("indexed_colors entries need an integer index")
        index = entry["index"]
        if not 16 <= index <= 255:
            raise ValueError("indexed_colors indices must be 16 through 255")
        palette[index] = color(entry.get("color"), f"indexed_colors[{index}].color")

    foreground = color(tables["primary"].get("foreground", DEFAULTS["primary"]["foreground"]),
                       "colors.primary.foreground")
    background = color(tables["primary"].get("background", DEFAULTS["primary"]["background"]),
                       "colors.primary.background")

    def cursor_color(key, default):
        value = tables["cursor"].get(key, default)
        if value == "CellForeground":
            return foreground
        if value == "CellBackground":
            return background
        return color(value, f"colors.cursor.{key}")

    palette[256] = cursor_color("cursor", "CellForeground")
    palette[257] = cursor_color("text", "CellBackground")
    palette[258] = foreground
    palette[259] = background
    palette[260] = color(tables["primary"].get("bright_foreground", foreground),
                         "colors.primary.bright_foreground")
    # Alacritty uses its fixed DimColors defaults for missing fields when a dim
    # table exists; without one, it derives the whole table from normal colors.
    for index, name in enumerate(NAMES, 261):
        fallback = DIM_DEFAULTS[name] if tables["dim"] else dim_color(palette[index - 261])
        palette[index] = color(tables["dim"].get(name, fallback), f"colors.dim.{name}")
    palette[269] = color(tables["primary"].get("dim_foreground", dim_color(foreground)),
                         "colors.primary.dim_foreground")

    bold_bright = colors.get("draw_bold_text_with_bright_colors",
                             config.get("draw_bold_text_with_bright_colors", False))
    if type(bold_bright) is not bool:
        raise ValueError("colors.draw_bold_text_with_bright_colors must be boolean")
    lines = ["/* Generated by manage.py from Alacritty config. Do not edit. */",
             f"static const int theme_font_offset_y = {line_offset};",
             f"static const int theme_glyph_offset_y = {glyph_offset_y};",
             f"static const double theme_font_size = {float(font_size)!r};",
             f"static const char *theme_font_family = {json.dumps(family, ensure_ascii=False) if family is not None else 'NULL'};",
             f"static const int theme_history_size = {history};",
             f"static const int theme_scroll_multiplier = {multiplier};",
             f"static const int theme_bold_bright = {int(bold_bright)};",
             "static const char *colorname[] = {"]
    lines += [f"\t[{index}] = {json.dumps(value)}," for index, value in sorted(palette.items())]
    lines += ["};", ""]
    return "\n".join(lines)


def generate_theme(config_path=None, output=OUTPUT):
    path = config_path if config_path is not None else find_config()
    config = read_config(path) if path else {}
    result = theme_header(config)
    output = Path(output)
    output.parent.mkdir(exist_ok=True)
    if not output.exists() or output.read_text() != result:
        output.write_text(result)
