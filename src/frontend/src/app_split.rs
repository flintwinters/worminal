//! Split panes: every pane is a tab strip over a notebook of its own tabs, and a window with one pane is an
//! ordinary window. The split is per window and not persisted; the daemon only learns which tabs this view
//! shows (`Pane`), never the geometry.
use super::{App, LINK_SHIFT};
use crate::{
    layout::{Layout, Target, Zone},
    tabs::{Actions, Tabs},
};
use gtk::prelude::*;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use worminal_protocol::{worp::commands::CommandKind, WIRE_FEATURE_PANES};

type Rect = (f64, f64, f64, f64);
/// One tab strip above the notebook holding just its tabs.
pub(super) struct Pane {
    id: u32,
    widget: gtk::Box,
    pub(super) notebook: gtk::Notebook,
    pub(super) strip: Tabs,
}
/// The content area: the pane tree, whose leaves are pane ids.
pub(super) struct Panes {
    root: gtk::Overlay,
    host: gtk::Box,
    /// Region a hovering tab drag would claim, in `root` coordinates.
    marked: Rc<Cell<Option<Rect>>>,
    mark: gtk::DrawingArea,
    list: RefCell<Vec<Rc<Pane>>>,
    layout: RefCell<Layout>,
    focused: Cell<u32>,
    next: Cell<u32>,
    /// Tabs last sent to the daemons as shown, so unchanged sets generate no traffic.
    presented: RefCell<Vec<u32>>,
}
impl Panes {
    pub(super) fn new() -> Self {
        let host = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let root = gtk::Overlay::new();
        root.add(&host);
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
            host,
            marked,
            mark,
            list: RefCell::new(Vec::new()),
            layout: RefCell::new(Layout::Leaf(0)),
            focused: Cell::new(0),
            next: Cell::new(0),
            presented: RefCell::new(Vec::new()),
        }
    }
    pub(super) fn widget(&self) -> &gtk::Overlay {
        &self.root
    }
}
/// Take a widget out of whatever holds it.
fn detach(widget: &gtk::Widget) {
    if let Some(parent) = widget.parent() {
        if let Ok(parent) = parent.downcast::<gtk::Container>() {
            parent.remove(widget);
        }
    }
}
impl App {
    /// The window's first pane; it holds every tab until one is dragged out.
    pub(super) fn start_panes(self: &Rc<Self>) {
        let pane = self.add_pane();
        self.panes.focused.set(pane.id);
        *self.panes.layout.borrow_mut() = Layout::Leaf(pane.id);
        self.rebuild();
    }
    fn add_pane(self: &Rc<Self>) -> Rc<Pane> {
        let id = self.panes.next.replace(self.panes.next.get() + 1);
        let notebook = gtk::Notebook::new();
        notebook.set_show_tabs(false);
        notebook.set_show_border(false);
        let strip = Tabs::new(self.fonts.clone());
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.pack_start(strip.widget(), false, false, 0);
        widget.pack_start(&notebook, true, true, 0);
        // GTK maps the new child in its default switch handler. Focus it afterwards.
        notebook.connect_local("switch-page", true, glib::clone!(@weak self as app => @default-return None, move |values| {
            if app.syncing.get() { return None; }
            let widget = values[1].get::<gtk::Widget>().ok()?;
            let id = app.pages.borrow().iter().find(|page| page.widget == widget).map(|page| page.id);
            if let Some(id) = id { app.activate(id); }
            None
        }));
        strip.connect(Actions {
            select: Box::new(glib::clone!(@weak self as app => move |id| app.select_id(id))),
            close: Box::new(
                glib::clone!(@weak self as app => move |id| app.send(id, CommandKind::Close { receipt: false })),
            ),
            moved: Box::new(glib::clone!(@weak self as app => move |id, target| app.move_tab(id, target))),
            locate: Box::new(
                glib::clone!(@weak self as app => @default-return None, move |id, x, y| app.locate(id, x, y)),
            ),
            hover: Box::new(glib::clone!(@weak self as app => move |target| app.mark(target))),
            split: Box::new(glib::clone!(@weak self as app => move |id, target| app.drop_tab(id, target))),
        });
        let pane = Rc::new(Pane {
            id,
            widget,
            notebook,
            strip,
        });
        self.panes.list.borrow_mut().push(pane.clone());
        pane
    }
    fn pane(&self, id: u32) -> Option<Rc<Pane>> {
        self.panes
            .list
            .borrow()
            .iter()
            .find(|pane| pane.id == id)
            .cloned()
    }
    pub(super) fn focused_pane(&self) -> Rc<Pane> {
        self.pane(self.panes.focused.get())
            .or_else(|| self.panes.list.borrow().first().cloned())
            .expect("a window always has a pane")
    }
    pub(super) fn notebook(&self) -> gtk::Notebook {
        self.focused_pane().notebook.clone()
    }
    pub(super) fn page_widget(&self, id: u32) -> Option<gtk::Widget> {
        self.pages
            .borrow()
            .iter()
            .find(|page| page.id == id)
            .map(|page| page.widget.clone())
    }
    pub(super) fn pane_of(&self, tab: u32) -> Option<Rc<Pane>> {
        let widget = self.page_widget(tab)?;
        self.panes
            .list
            .borrow()
            .iter()
            .find(|pane| pane.notebook.page_num(&widget).is_some())
            .cloned()
    }
    /// The pane's selected tab.
    pub(super) fn active_of(&self, pane: &Pane) -> Option<u32> {
        let widget = pane.notebook.nth_page(pane.notebook.current_page())?;
        self.pages
            .borrow()
            .iter()
            .find(|page| page.widget == widget)
            .map(|page| page.id)
    }
    /// The pane's tabs in daemon order.
    pub(super) fn tabs_of(&self, pane: &Pane) -> Vec<u32> {
        self.pages
            .borrow()
            .iter()
            .filter(|page| pane.notebook.page_num(&page.widget).is_some())
            .map(|page| page.id)
            .collect()
    }
    pub(super) fn panes(&self) -> Vec<Rc<Pane>> {
        self.panes.list.borrow().clone()
    }
    /// The strip showing a tab (the focused one for a tab that has no pane yet).
    pub(super) fn strip_of(&self, tab: u32) -> Tabs {
        self.pane_of(tab)
            .unwrap_or_else(|| self.focused_pane())
            .strip
            .clone()
    }
    /// The selected tab of each pane, in reading order.
    fn visible(&self) -> Vec<u32> {
        let leaves = self.panes.layout.borrow().leaves();
        leaves
            .into_iter()
            .filter_map(|id| self.active_of(&*self.pane(id)?))
            .collect()
    }
    /// A tab on screen paints and resizes, so it needs its own geometry sent.
    pub(super) fn is_visible(&self, id: u32) -> bool {
        id == self.active.get() || self.visible().contains(&id)
    }
    /// Only terminals whose daemon can keep several tabs shown take part in a split.
    fn can_split(&self, id: u32) -> bool {
        self.terminal(id).is_some() && self.features(id) & WIRE_FEATURE_PANES != 0
    }
    pub(super) fn focus_pane(&self, pane: &Pane) {
        self.panes.focused.set(pane.id);
    }
    /// Focus the pane holding a tab and select its page there.
    pub(super) fn show_page(&self, id: u32) {
        let (Some(pane), Some(widget)) = (self.pane_of(id), self.page_widget(id)) else {
            return;
        };
        self.focus_pane(&pane);
        pane.notebook
            .set_current_page(pane.notebook.page_num(&widget));
    }
    /// Where a drop of `dragged` at this position (in its own pane's strip) would land: another strip or the
    /// middle of another pane moves it there, a pane's edge splits that pane. Its own strip only reorders.
    pub(super) fn locate(&self, dragged: u32, x: f64, y: f64) -> Option<Target> {
        if !self.can_split(dragged) {
            return None;
        }
        let home = self.pane_of(dragged)?;
        let alone = self.tabs_of(&home).len() < 2;
        let from = home.strip.widget().clone();
        self.panes().into_iter().find_map(|pane| {
            let own = home.id == pane.id;
            let strip = pane.strip.widget();
            if let Some((sx, sy)) = from.translate_coordinates(strip, x as i32, y as i32) {
                if (0..strip.allocated_width()).contains(&sx)
                    && (0..strip.allocated_height()).contains(&sy)
                {
                    return (!own).then_some(Target {
                        anchor: pane.id,
                        zone: Zone::Center,
                    });
                }
            }
            let content = &pane.notebook;
            let (cx, cy) = from.translate_coordinates(content, x as i32, y as i32)?;
            let zone = Zone::at(
                f64::from(cx),
                f64::from(cy),
                f64::from(content.allocated_width()),
                f64::from(content.allocated_height()),
            )?;
            // A pane's only tab cannot split itself, and its own middle holds it already.
            (!(own && (alone || zone == Zone::Center))).then_some(Target {
                anchor: pane.id,
                zone,
            })
        })
    }
    /// Mark the region a drag would claim until it moves elsewhere.
    pub(super) fn mark(&self, target: Option<Target>) {
        let rect = target.and_then(|target| {
            let pane = self.pane(target.anchor)?;
            let widget: gtk::Widget = if target.zone == Zone::Center {
                pane.widget.clone().upcast()
            } else {
                pane.notebook.clone().upcast()
            };
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
    /// Move a tab's page into `dest`, leaving the pane that held it.
    fn move_page(&self, id: u32, dest: &Pane) {
        let (Some(widget), Some(source)) = (self.page_widget(id), self.pane_of(id)) else {
            return;
        };
        if source.id == dest.id {
            return;
        }
        let label = self
            .pages
            .borrow()
            .iter()
            .find(|page| page.id == id)
            .map(|page| page.label.clone());
        let was = self.syncing.replace(true);
        if let Some(index) = source.notebook.page_num(&widget) {
            source.notebook.remove_page(Some(index));
        }
        dest.notebook.append_page(&widget, label.as_ref());
        widget.show();
        self.syncing.set(was);
    }
    /// A tab dropped on a pane joins it, or on a pane edge gets a new pane beside it; it takes focus.
    pub(super) fn drop_tab(self: &Rc<Self>, id: u32, target: Target) {
        let Some(anchor) = self.pane(target.anchor) else {
            return;
        };
        if target.zone == Zone::Center {
            self.move_page(id, &anchor);
        } else {
            let beside = self.add_pane();
            self.move_page(id, &beside);
            let layout = self.panes.layout.borrow().clone();
            *self.panes.layout.borrow_mut() = layout.insert(anchor.id, beside.id, target.zone);
        }
        self.reorder_panes();
        self.collapse();
        self.rebuild();
        self.select_id(id);
    }
    /// Gather every tab into the focused pane again.
    pub(super) fn unsplit(self: &Rc<Self>) {
        let home = self.focused_pane();
        let tabs: Vec<_> = self.pages.borrow().iter().map(|page| page.id).collect();
        for id in tabs {
            self.move_page(id, &home);
        }
        self.reorder_panes();
        self.collapse();
        self.rebuild();
        self.select_id(self.active.get());
    }
    /// Keep each notebook in daemon order.
    pub(super) fn reorder_panes(&self) {
        for pane in self.panes() {
            for (position, id) in self.tabs_of(&pane).into_iter().enumerate() {
                if let Some(widget) = self.page_widget(id) {
                    pane.notebook.reorder_child(&widget, Some(position as u32));
                }
            }
        }
    }
    /// Panes left without tabs give their space to a sibling. Reports whether the layout changed.
    pub(super) fn collapse(&self) -> bool {
        let mut changed = false;
        for pane in self.panes() {
            if self.panes.list.borrow().len() > 1 && pane.notebook.n_pages() == 0 {
                let layout = self.panes.layout.borrow().clone();
                if let Some(rest) = layout.without(pane.id) {
                    *self.panes.layout.borrow_mut() = rest;
                }
                self.panes
                    .list
                    .borrow_mut()
                    .retain(|other| other.id != pane.id);
                changed = true;
            }
        }
        if changed && self.pane(self.panes.focused.get()).is_none() {
            let first = self.panes.layout.borrow().leaves()[0];
            self.panes.focused.set(first);
        }
        changed
    }
    /// Rebuild the widget tree. Pane widgets leave the old tree first: GTK destroys a container's children
    /// with it, and a pane holds live terminals.
    pub(super) fn rebuild(&self) {
        for pane in self.panes() {
            detach(pane.widget.upcast_ref());
        }
        for child in self.panes.host.children() {
            self.panes.host.remove(&child);
        }
        let layout = self.panes.layout.borrow().clone();
        self.panes
            .host
            .pack_start(&self.tree(&layout), true, true, 0);
        self.panes.host.show_all();
        self.panes.mark.queue_draw();
    }
    fn tree(&self, layout: &Layout) -> gtk::Widget {
        match layout {
            Layout::Leaf(id) => self.pane(*id).map_or_else(
                || gtk::Box::new(gtk::Orientation::Vertical, 0).upcast(),
                |pane| pane.widget.clone().upcast(),
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
    /// Tell each daemon which tabs this window shows. `Focus` leaves one tab shown per daemon, so the first
    /// shown tab of each daemon takes it and the others join; an unchanged set is sent again only if forced.
    pub(super) fn present(self: &Rc<Self>, force: bool) {
        let active = self.active.get();
        let mut shown = self.visible();
        shown.retain(|id| self.terminal(*id).is_some());
        shown.sort_by_key(|id| *id != active);
        if !force && *self.panes.presented.borrow() == shown {
            return;
        }
        self.panes.presented.replace(shown.clone());
        if shown.len() < 2 {
            shown = vec![active];
        }
        let mut focused = Vec::new();
        for id in shown {
            let Some(terminal) = self.terminal(id) else {
                continue;
            };
            let (columns, rows) = terminal.dimensions();
            let link = id >> LINK_SHIFT;
            let join = focused.contains(&link) && self.features(id) & WIRE_FEATURE_PANES != 0;
            focused.push(link);
            self.send(
                id,
                if join {
                    CommandKind::Pane { columns, rows }
                } else {
                    CommandKind::Focus { columns, rows }
                },
            );
        }
    }
}
