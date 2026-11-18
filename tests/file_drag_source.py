"""Separate GTK application offering a file-manager-compatible URI-list drag."""
import sys

import gi
gi.require_version('Gtk', '3.0')
from gi.repository import Gtk, Gdk


window = Gtk.Window(title='External file drag source')
window.set_default_size(180, 100)
source = Gtk.EventBox()
source.add(Gtk.Label(label='Drag files'))
source.drag_source_set(Gdk.ModifierType.BUTTON1_MASK,
                       [Gtk.TargetEntry.new('text/uri-list', 0, 0)], Gdk.DragAction.COPY)
def provide(_widget, _context, data, _info, _time):
    print('data requested', flush=True)
    data.set_uris(sys.argv[1:])


source.connect('drag-data-get', provide)
source.connect('drag-begin', lambda *_args: print('drag began', flush=True))
source.connect('drag-failed', lambda _widget, _context, result: print(f'drag failed: {result}', flush=True))
window.add(source)
window.connect('destroy', Gtk.main_quit)
window.show_all()
Gtk.main()
