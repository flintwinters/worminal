//! Split panes: dragging a tab onto a pane shows both terminals in one window. The split is per window and not
//! persisted; the daemon only learns which tabs this view shows (`Pane`), never the geometry.
use super::{App, LINK_SHIFT};
use crate::{
    layout::{Layout, Target, Zone},
    terminal::Terminal,
};
use gtk::prelude::*;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use worminal_protocol::{worp::commands::CommandKind, WIRE_FEATURE_PANES};

type Rect = (f64, f64, f64, f64);
/// The content area: the notebook of single tabs, or the pane tree while the window is split.
pub(super) struct Panes {
    root: gtk::Overlay,
    stack: gtk::Stack,
    host: gtk::Box,
    /// Region a hovering tab drag would claim, in `root` coordinates.
    marked: Rc<Cell<Option<Rect>>>,
    mark: gtk::DrawingArea,
    layout: RefCell<Option<Layout>>,
    /// The shown set changed since the daemon was told, so its other shown tabs must be hidden.
    fresh: Cell<bool>,
}
impl Panes {
    pub(super) fn new(notebook: &gtk::Notebook) -> Self {
        let stack = gtk::Stack::new();
        let host = gtk::Box::new(gtk::Orientation::Vertical, 0);
        // A stack shows the first visible child it is given.
        notebook.show();
        host.show();
        stack.add(notebook);
        stack.add(&host);
        stack.set_visible_child(notebook);
        let root = gtk::Overlay::new();
        root.add(&stack);
        let mark = gtk::DrawingArea::new();
        // A windowed overlay would intercept pointer events above terminal content.
        mark.set_has_window(false);
        root.add_overlay(&mark);
        root.set_overlay_pass_through(&mark, true);
        let marked = Rc::new(Cell::new(None::<Rect>));
        mark.connect_draw(
            glib::clone!(@strong marked => @default-return glib::Propagation::Proceed,
                move |_, cr| {
                if let Some((x, y, width, height)) = marked.get() {
                    cr.rectangle(x, y, width, height);
                    cr.set_source_rgba(0.45, 0.65, 1.0, 0.25);
                    if let Err(error) = cr.fill_preserve() {
                        eprintln!("worminal: split mark: {error}");
                    }
                    cr.set_source_rgba(0.45, 0.65, 1.0, 0.9);
                    cr.set_line_width(2.0);
                    if let Err(error) = cr.stroke() {
                        eprintln!("worminal: split mark: {error}");
                    }
                }
                glib::Propagation::Proceed
            }),
        );
        Self {
            root,
            stack,
            host,
            marked,
            mark,
            layout: RefCell::new(None),
            fresh: Cell::new(false),
        }
    }
    pub(super) fn widget(&self) -> &gtk::Overlay {
        &self.root
    }
    fn leaves(&self) -> Option<Vec<u32>> {
        self.layout.borrow().as_ref().map(Layout::leaves)
    }
}
/// Move a widget under `parent`, leaving whatever held it.
fn adopt(widget: &gtk::Widget, parent: &gtk::Box, expand: bool) {
    if let Some(old) = widget.parent() {
        if let Ok(old) = old.downcast::<gtk::Container>() {
            old.remove(widget);
        }
    }
    parent.pack_start(widget, expand, expand, 0);
}
impl App {
    /// Route tab strip clicks and drags to this window.
    pub(super) fn connect_tabstrip(self: &Rc<Self>) {
        let app = self;
        self.tabstrip.connect(crate::tabs::Actions {
            select: Box::new(glib::clone!(@weak app => move |id| app.select_id(id))),
            close: Box::new(
                glib::clone!(@weak app => move |id| app.send(id, CommandKind::Close { receipt: false })),
            ),
            moved: Box::new(glib::clone!(@weak app => move |id, target| app.move_tab(id, target))),
            locate: Box::new(
                glib::clone!(@weak app => @default-return None, move |id, x, y| app.locate(id, x, y)),
            ),
            hover: Box::new(glib::clone!(@weak app => move |target| app.mark(target))),
            split: Box::new(glib::clone!(@weak app => move |id, target| app.split(id, target))),
        });
    }
    /// Terminals on screen: the split's panes, or the selected tab alone.
    fn visible(&self) -> Vec<(u32, Terminal)> {
        let ids = self
            .panes
            .leaves()
            .unwrap_or_else(|| vec![self.active.get()]);
        ids.into_iter()
            .filter_map(|id| Some((id, self.terminal(id)?)))
            .collect()
    }
    /// A tab on screen paints and resizes, so it needs its own geometry sent.
    pub(super) fn is_visible(&self, id: u32) -> bool {
        id == self.active.get()
            || self
                .panes
                .leaves()
                .is_some_and(|leaves| leaves.contains(&id))
    }
    /// Only terminals whose daemon can keep several tabs shown take part in a split.
    fn can_split(&self, id: u32) -> bool {
        self.terminal(id).is_some() && self.features(id) & WIRE_FEATURE_PANES != 0
    }
    /// The pane region a drop of `dragged` at this tab strip position would claim.
    pub(super) fn locate(&self, dragged: u32, x: f64, y: f64) -> Option<Target> {
        if !self.can_split(dragged) {
            return None;
        }
        let strip = self.tabstrip.widget();
        self.visible().into_iter().find_map(|(anchor, terminal)| {
            let widget = terminal.widget();
            if anchor == dragged {
                return None;
            }
            let (x, y) = strip.translate_coordinates(widget, x as i32, y as i32)?;
            let size = (
                f64::from(widget.allocated_width()),
                f64::from(widget.allocated_height()),
            );
            Some(Target {
                anchor,
                zone: Zone::at(f64::from(x), f64::from(y), size.0, size.1)?,
            })
        })
    }
    /// Mark the region a drag would claim until it moves elsewhere.
    pub(super) fn mark(&self, target: Option<Target>) {
        let rect = target.and_then(|target| {
            let widget = self.terminal(target.anchor)?.widget().clone();
            let (x, y) = widget.translate_coordinates(self.panes.widget(), 0, 0)?;
            let (dx, dy, width, height) = target.zone.rect(
                f64::from(widget.allocated_width()),
                f64::from(widget.allocated_height()),
            );
            Some((f64::from(x) + dx, f64::from(y) + dy, width, height))
        });
        self.panes.marked.set(rect);
        self.panes.mark.queue_draw();
    }
    /// A tab dropped on a pane joins the layout there (a layout of one pane is no split) and takes focus.
    pub(super) fn split(self: &Rc<Self>, dragged: u32, target: Target) {
        let layout = self
            .panes
            .layout
            .borrow()
            .clone()
            .unwrap_or(Layout::Leaf(target.anchor))
            .without(dragged)
            .unwrap_or(Layout::Leaf(target.anchor));
        self.set_layout(Some(layout.insert(target.anchor, dragged, target.zone)));
        self.select_id(dragged);
    }
    /// Show only the focused pane's tab again.
    pub(super) fn unsplit(self: &Rc<Self>) {
        if self.panes.leaves().is_some() {
            self.set_layout(None);
            self.activate(self.active.get());
        }
    }
    /// A closed tab leaves its pane; the sibling takes the space.
    pub(super) fn leave(&self, id: u32) {
        let layout = self.panes.layout.borrow().clone();
        if let Some(layout) = layout.filter(|layout| layout.contains(id)) {
            let remaining = layout.without(id);
            // The pane left behind inherits focus, so the catalog's fallback does not pick another tab.
            if id == self.active.get() {
                if let Some(next) = remaining
                    .as_ref()
                    .and_then(|layout| layout.leaves().first().copied())
                {
                    self.active.set(next);
                }
            }
            self.set_layout(remaining);
        }
    }
    /// Selecting a tab outside the panes puts it in the focused pane, or shows it alone if it cannot be split.
    pub(super) fn place(&self, id: u32) {
        let layout = self.panes.layout.borrow().clone();
        let Some(layout) = layout.filter(|layout| !layout.contains(id)) else {
            return;
        };
        let focused = self.active.get();
        self.set_layout(
            Some(layout)
                .filter(|layout| self.can_split(id) && layout.contains(focused))
                .map(|layout| layout.insert(focused, id, Zone::Center)),
        );
    }
    fn set_layout(&self, layout: Option<Layout>) {
        *self.panes.layout.borrow_mut() = layout.and_then(Layout::split_only);
        self.panes.fresh.set(true);
        // Terminal widgets are destroyed with the container that holds them, so take each out before
        // discarding the old tree.
        let terminals: Vec<_> = self
            .pages
            .borrow()
            .iter()
            .filter_map(|page| Some((page.id, page.terminal.clone()?, page.widget.clone())))
            .collect();
        for (_, terminal, _) in &terminals {
            if let Some(parent) = terminal.widget().parent() {
                if let Ok(parent) = parent.downcast::<gtk::Container>() {
                    parent.remove(terminal.widget());
                }
            }
        }
        for child in self.panes.host.children() {
            self.panes.host.remove(&child);
        }
        let layout = self.panes.layout.borrow().clone();
        for (id, terminal, wrapper) in &terminals {
            if layout.as_ref().is_none_or(|layout| !layout.contains(*id)) {
                if let Ok(wrapper) = wrapper.clone().downcast::<gtk::Box>() {
                    adopt(terminal.widget().upcast_ref(), &wrapper, true);
                }
            }
        }
        match layout {
            Some(layout) => {
                self.panes
                    .host
                    .pack_start(&self.tree(&layout), true, true, 0);
                self.panes.host.show_all();
                self.panes.stack.set_visible_child(&self.panes.host);
            }
            None => self.panes.stack.set_visible_child(&self.notebook),
        }
        self.panes.mark.queue_draw();
    }
    fn tree(&self, layout: &Layout) -> gtk::Widget {
        match layout {
            Layout::Leaf(id) => self.terminal(*id).map_or_else(
                || gtk::Box::new(gtk::Orientation::Vertical, 0).upcast(),
                |terminal| terminal.widget().clone().upcast(),
            ),
            Layout::Split {
                side_by_side,
                first,
                second,
            } => {
                let side_by_side = *side_by_side;
                let paned = gtk::Paned::new(if side_by_side {
                    gtk::Orientation::Horizontal
                } else {
                    gtk::Orientation::Vertical
                });
                paned.pack1(&self.tree(first), true, false);
                paned.pack2(&self.tree(second), true, false);
                // The divider starts in the middle; GTK would size it from the panes' tiny minimum requests.
                let placed = Cell::new(false);
                paned.connect_size_allocate(move |paned, allocation| {
                    let size = if side_by_side {
                        allocation.width()
                    } else {
                        allocation.height()
                    };
                    if size > 1 && !placed.replace(true) {
                        let weak = paned.downgrade();
                        glib::idle_add_local_once(move || {
                            if let Some(paned) = weak.upgrade() {
                                paned.set_position(size / 2);
                            }
                        });
                    }
                });
                paned.upcast()
            }
        }
    }
    /// Tell each daemon which tabs this window shows. `Focus` leaves one tab shown per daemon, so after the
    /// shown set changed the first pane of each daemon takes it and the others join; otherwise the focused
    /// pane only reclaims control.
    pub(super) fn present(self: &Rc<Self>) {
        let active = self.active.get();
        let Some(mut leaves) = self.panes.leaves() else {
            if let Some(terminal) = self.terminal(active) {
                let (columns, rows) = terminal.dimensions();
                self.send(active, CommandKind::Focus { columns, rows });
            }
            return;
        };
        let fresh = self.panes.fresh.replace(false);
        if !fresh {
            leaves.retain(|id| *id == active);
        }
        leaves.sort_by_key(|id| *id != active);
        let mut focused = Vec::new();
        for id in leaves {
            let Some(terminal) = self.terminal(id) else {
                continue;
            };
            let (columns, rows) = terminal.dimensions();
            let link = id >> LINK_SHIFT;
            let join = !fresh || focused.contains(&link);
            focused.push(link);
            self.send(
                id,
                if join && self.features(id) & WIRE_FEATURE_PANES != 0 {
                    CommandKind::Pane { columns, rows }
                } else {
                    CommandKind::Focus { columns, rows }
                },
            );
        }
    }
}
