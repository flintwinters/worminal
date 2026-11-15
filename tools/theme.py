"""Runtime configuration for tools outside Rust.

`worminald config --json` is the only resolver (worminal.yaml layered over Alacritty config); this module
asks it for the answer and adapts the result for the explorer build.
"""

import ctypes
import json
import os
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent


def settings(defaults=False, environment=None):
    """Resolved configuration; `defaults` ignores every file and reports the built-in values.

    `environment` overlays variables for the resolver, e.g. to select one Alacritty file."""
    from tools.build import Builder
    daemon = Builder().rust_artifact("worminal-daemon")
    arguments = [str(daemon), "config", "--json", *(["--defaults"] if defaults else [])]
    result = subprocess.run(arguments, capture_output=True, text=True, check=False,
                            env={**os.environ, **(environment or {})})
    if result.returncode:
        raise ValueError(result.stderr.strip() or "worminald config failed")
    return json.loads(result.stdout)


def explorer_font():
    """Export the terminal's configured font as CSS build input."""
    values = settings()
    # Match Font::resolve: absent families use Pango's default, and sizes clamp to 4pt.
    result = {"family": values["font_family"] or "sans", "size": max(4, values["font_size"])}
    result["lineHeight"] = max(1, font_height(result["family"], result["size"]) + values["font_offset_y"])
    return result


def font_height(family, size):
    """Match terminal_font: 96 DPI, separately rounded Pango ascent and descent."""
    pango = ctypes.CDLL("libpango-1.0.so.0")
    cairo = ctypes.CDLL("libpangocairo-1.0.so.0")
    gobject = ctypes.CDLL("libgobject-2.0.so.0")

    def bind(library, name, result, *arguments):
        function = getattr(library, name)
        function.restype = result
        function.argtypes = arguments
        return function

    pointer = ctypes.c_void_p
    integer = ctypes.c_int
    font_map = bind(cairo, "pango_cairo_font_map_get_default", pointer)()
    context = bind(pango, "pango_font_map_create_context", pointer, pointer)(font_map)
    font = bind(pango, "pango_font_description_new", pointer)()
    metrics = None
    try:
        bind(cairo, "pango_cairo_context_set_resolution", None, pointer, ctypes.c_double)(context, 96)
        bind(pango, "pango_font_description_set_family", None, pointer, ctypes.c_char_p)(font, family.encode())
        bind(pango, "pango_font_description_set_size", None, pointer, integer)(font, int(max(4, size) * 1024))
        language = bind(pango, "pango_context_get_language", pointer, pointer)(context)
        metrics = bind(pango, "pango_context_get_metrics", pointer, pointer, pointer, pointer)(context, font, language)
        return sum((bind(pango, f"pango_font_metrics_get_{part}", integer, pointer)(metrics) + 1023) // 1024
                   for part in ("ascent", "descent"))
    finally:
        if metrics:
            bind(pango, "pango_font_metrics_unref", None, pointer)(metrics)
        bind(pango, "pango_font_description_free", None, pointer)(font)
        bind(gobject, "g_object_unref", None, pointer)(context)
