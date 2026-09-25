"""Minimal window manager configuration for the private X11 proof."""

from libqtile import layout
from libqtile.config import Group, Screen

groups = [Group("proof")]
layouts = [layout.Max()]
screens = [Screen()]
keys = []
mouse = []
floating_layout = layout.Floating()
