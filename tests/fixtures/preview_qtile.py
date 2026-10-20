"""Keep both preview windows visible while exercising independent browser pages."""
from libqtile import layout
from libqtile.config import Group, Screen

groups = [Group("preview")]
layouts = [layout.Columns()]
screens = [Screen()]
keys = []
mouse = []
floating_layout = layout.Floating()
