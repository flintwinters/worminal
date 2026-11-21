//! Split panes: every pane is a tab strip over a notebook of its own tabs, and a window with one pane is an
//! ordinary window. The split is per window and remembered between runs (`app_layout.rs`); the daemon only learns which tabs this view
//! shows (`Pane`), never the geometry.
use super::{App, LINK_SHIFT};
use crate::{
    layout::{neighbor, Direction, Layout, PaneRect, Target, Zone},
    paint,
    tabs::{mix, Actions, Ghost, Landing, Tabs},
};
use gtk::prelude::*;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use worminal_protocol::{
    cell::{COLOR_DEFAULT_BACKGROUND as BG, COLOR_DEFAULT_FOREGROUND as FG},
    worp::commands::CommandKind,
    WIRE_FEATURE_PANES,
};
use worminal_workspace::layout_state::RATIO_FULL;

type Rect = (f64, f64, f64, f64);
/// One tab strip above the notebook holding just its tabs.
pub(super) struct Pane {
    pub(super) id: u32,
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
    /// The dragged tab's image and its top-left, in `root` coordinates.
    ghost: Rc<RefCell<Option<(cairo::ImageSurface, f64, f64)>>>,
    pub(super) list: RefCell<Vec<Rc<Pane>>>,
    pub(super) layout: RefCell<Layout>,
    pub(super) focused: Cell<u32>,
    next: Cell<u32>,
    /// Tabs last sent to the daemons as shown, so unchanged sets generate no traffic.
    presented: RefCell<Vec<u32>>,
    /// What the window remembers of its layout between runs.
    pub(super) persist: super::layout_state::Persist,
}
impl Panes {
    pub(super) fn new() -> Self {
        let host = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let root = gtk::Overlay::new();
        root.add(&host);
        // Draw after children instead of using an overlay widget: GTK drag hit testing
        // ignores overlay pointer pass-through and would hide terminal drop targets.
        let marked = Rc::new(Cell::new(None::<Rect>));
        let ghost = Rc::new(RefCell::new(None::<(cairo::ImageSurface, f64, f64)>));
        root.connect_local(
            "draw",
            true,
            glib::clone!(@strong marked, @strong ghost => move |values| {
                let cr = values[1].get::<cairo::Context>().unwrap();
                if let Some((x, y, width, height)) = marked.get() {
                    cr.rectangle(x, y, width, height);
                    cr.set_source_rgba(0.6, 0.6, 0.6, 0.25);
                    if let Err(error) = cr.fill_preserve() {
                        eprintln!("worminal: split mark: {error}");
                    }
                    cr.set_source_rgba(0.6, 0.6, 0.6, 0.9);
                    cr.set_line_width(2.0);
                    if let Err(error) = cr.stroke() {
                        eprintln!("worminal: split mark: {error}");
                    }
                }
                if let Some((image, x, y)) = ghost.borrow().as_ref() {
                    if let Err(error) = cr
                        .set_source_surface(image, *x, *y)
                        .and_then(|()| cr.paint_with_alpha(0.6))
                    {
                        eprintln!("worminal: dragged tab: {error}");
                    }
                }
                Some(false.to_value())
            }),
        );
        Self {
            root,
            host,
            marked,
            ghost,
            list: RefCell::new(Vec::new()),
            layout: RefCell::new(Layout::Leaf(0)),
            focused: Cell::new(0),
            next: Cell::new(0),
            presented: RefCell::new(Vec::new()),
            persist: super::layout_state::Persist::default(),
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
        self.focus_pane(&pane);
        *self.panes.layout.borrow_mut() = Layout::Leaf(pane.id);
        // After the children, like the drop highlight: an overlay widget would hide drop targets from GTK.
        self.panes.root.connect_local(
            "draw",
            true,
            glib::clone!(@weak self as app => @default-return None, move |values| {
                let cr = values[1].get::<gtk::cairo::Context>().ok()?;
                if let Err(error) = app.draw_borders(&cr) {
                    eprintln!("worminal: pane borders: {error}");
                }
                Some(false.to_value())
            }),
        );
        self.rebuild();
    }
    /// Outline every pane of a split in the terminal's colors, the focused one brighter on top, so the panes
    /// read as separate terminals and the one taking keystrokes is plain. A window with one pane has none.
    fn draw_borders(&self, cr: &gtk::cairo::Context) -> Result<(), gtk::cairo::Error> {
        let panes = self.panes();
        if panes.len() < 2 {
            return Ok(());
        }
        let palette = self
            .pages
            .borrow()
            .iter()
            .find(|page| page.id == self.active.get())
            .map(|page| page.palette.clone());
        let Some(palette) = palette else {
            return Ok(());
        };
        let (background, foreground) = {
            let palette = palette.borrow();
            (palette.color(BG), palette.color(FG))
        };
        let focused = self.panes.focused.get();
        let rects = self.pane_rects();
        cr.set_line_width(1.0);
        // Unfocused outlines first: the focused one covers the edges it shares.
        for (focus, amount) in [(false, 0.3), (true, 0.75)] {
            paint::source(cr, &mix(&background, &foreground, amount));
            for &(id, (x, y, width, height)) in &rects {
                if (id == focused) == focus {
                    cr.rectangle(
                        f64::from(x) + 0.5,
                        f64::from(y) + 0.5,
                        f64::from(width - 1),
                        f64::from(height - 1),
                    );
                    cr.stroke()?;
                }
            }
        }
        Ok(())
    }
    pub(super) fn add_pane(self: &Rc<Self>) -> Rc<Pane> {
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
        let strip_widget = strip.widget().clone();
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
            land: Box::new(glib::clone!(@weak self as app => move |id, target| app.drop_tab(id, target))),
            ghost: Box::new(glib::clone!(@weak self as app, @weak strip_widget => move |ghost| {
                app.ghost(&strip_widget, ghost)
            })),
            star: Box::new(glib::clone!(@weak self as app => move |id| app.toggle_bookmark(id))),
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
    pub(super) fn pane(&self, id: u32) -> Option<Rc<Pane>> {
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
    pub(super) fn visible(&self) -> Vec<u32> {
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
    /// Browser tabs always take part in a split; a terminal only if its daemon can keep several tabs shown.
    fn can_split(&self, id: u32) -> bool {
        // A browser tab has no PTY and no frames, so the daemon never has to show it beside anything.
        let browser = self
            .pages
            .borrow()
            .iter()
            .any(|page| page.id == id && page.browser);
        browser || (self.terminal(id).is_some() && self.features(id) & WIRE_FEATURE_PANES != 0)
    }
    /// The one place pane focus changes, so only the focused strip highlights its selected tab.
    pub(super) fn focus_pane(&self, pane: &Pane) {
        if self.panes.focused.replace(pane.id) != pane.id {
            self.panes.root.queue_draw();
        }
        for other in self.panes() {
            other.strip.focus(other.id == pane.id);
        }
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
    pub(super) fn locate(&self, dragged: u32, x: f64, y: f64) -> Option<Landing> {
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
                    let slot = pane.strip.slot(f64::from(sx), f64::from(sy));
                    return slot.filter(|_| !own).map(|slot| Landing::Strip {
                        pane: pane.id,
                        slot,
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
            (!(own && (alone || zone == Zone::Center))).then_some(Landing::Content(Target {
                anchor: pane.id,
                zone,
            }))
        })
    }
    /// Mark where a drag would land until it moves elsewhere: a slot in a strip, or a region of content.
    pub(super) fn mark(&self, landing: Option<Landing>) {
        for pane in self.panes() {
            pane.strip.hint(match landing {
                Some(Landing::Strip { pane: id, slot }) if id == pane.id => Some(slot),
                _ => None,
            });
        }
        let target = match landing {
            Some(Landing::Content(target)) => Some(target),
            _ => None,
        };
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
        self.panes.root.queue_draw();
    }
    /// Draw the dragged tab at its strip position, redrawing only the area it leaves and enters.
    fn ghost(&self, strip: &gtk::DrawingArea, ghost: Option<Ghost>) {
        let root = &self.panes.root;
        let placed = ghost.and_then(|ghost| {
            let (x, y) = strip.translate_coordinates(root, 0, 0)?;
            Some((ghost.image, f64::from(x) + ghost.x, f64::from(y) + ghost.y))
        });
        let old = self.panes.ghost.replace(placed);
        for (image, x, y) in old.iter().chain(self.panes.ghost.borrow().iter()) {
            let scale = f64::from(root.scale_factor());
            root.queue_draw_area(
                x.floor() as i32,
                y.floor() as i32,
                (f64::from(image.width()) / scale).ceil() as i32 + 1,
                (f64::from(image.height()) / scale).ceil() as i32 + 1,
            );
        }
    }
    /// Move a tab's page into `dest`, leaving the pane that held it.
    pub(super) fn move_page(&self, id: u32, dest: &Pane) {
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
    /// A tab dropped on a pane joins it (at the slot, on its strip), or on a pane edge gets a new pane beside
    /// it; it takes focus.
    pub(super) fn drop_tab(self: &Rc<Self>, id: u32, landing: Landing) {
        let (target, slot) = match landing {
            Landing::Strip { pane, slot } => (
                Target {
                    anchor: pane,
                    zone: Zone::Center,
                },
                Some(slot),
            ),
            Landing::Content(target) => (target, None),
        };
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
        if let Some(slot) = slot {
            self.move_tab(id, slot);
        }
    }
    /// The panes' rectangles in the pane area's coordinates.
    fn pane_rects(&self) -> Vec<(u32, PaneRect)> {
        self.panes()
            .iter()
            .filter_map(|pane| {
                let (x, y) = pane
                    .widget
                    .translate_coordinates(self.panes.widget(), 0, 0)?;
                Some((
                    pane.id,
                    (
                        x,
                        y,
                        pane.widget.allocated_width(),
                        pane.widget.allocated_height(),
                    ),
                ))
            })
            .collect()
    }
    /// Select the tab shown by the pane lying `direction` of the focused one. False when there is none, so
    /// a window without a split leaves the keystroke to the application.
    pub(super) fn focus_toward(self: &Rc<Self>, direction: Direction) -> bool {
        let from = self.focused_pane().id;
        let Some(pane) = neighbor(&self.pane_rects(), from, direction).and_then(|id| self.pane(id))
        else {
            return false;
        };
        if let Some(tab) = self.active_of(&pane) {
            self.select_id(tab);
        }
        true
    }
    /// Move the current tab as a drag would: into the pane lying `direction` of its own, or, at the window's
    /// edge, into a new pane split off that side (never emptying its own pane). False when the tab cannot
    /// move, so the keystroke stays the application's.
    pub(super) fn move_toward(self: &Rc<Self>, direction: Direction) -> bool {
        let id = self.active.get();
        let Some(home) = self.pane_of(id).filter(|_| self.can_split(id)) else {
            return false;
        };
        let target = match neighbor(&self.pane_rects(), home.id, direction) {
            Some(anchor) => Target {
                anchor,
                zone: Zone::Center,
            },
            None if self.tabs_of(&home).len() > 1 => Target {
                anchor: home.id,
                zone: direction.zone(),
            },
            None => return false,
        };
        self.drop_tab(id, Landing::Content(target));
        true
    }
    /// A terminal asks to become the current tab: it was pressed, or must take control. Panes of a split are all
    /// live, so liveness alone cannot tell whether the press changed the current tab. It must: typing follows
    /// the focused widget, but copy, paste, new tabs and pane focus follow the active tab, and a click on the
    /// other pane once typed there while Ctrl+Shift+C/V still used the previous tab.
    pub(super) fn claim(self: &Rc<Self>, id: u32) {
        if self.active.get() != id || !self.terminal(id).is_some_and(|terminal| terminal.live()) {
            self.activate(id);
        }
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
            if let Some(pane) = self.pane(first) {
                self.focus_pane(&pane);
            }
        }
        changed
    }
    /// Rebuild the widget tree. Pane widgets leave the old tree first: GTK destroys a container's children
    /// with it, and a pane holds live terminals.
    pub(super) fn rebuild(self: &Rc<Self>) {
        for pane in self.panes() {
            detach(pane.widget.upcast_ref());
        }
        for child in self.panes.host.children() {
            self.panes.host.remove(&child);
        }
        let layout = self.panes.layout.borrow().clone();
        self.panes
            .host
            .pack_start(&self.tree(&layout, &mut Vec::new()), true, true, 0);
        self.panes.host.show_all();
        self.panes.root.queue_draw();
    }
    /// The widgets of a layout. `path` addresses the split being built (`false` is a first child), so a moved
    /// divider can be written back to the layout that remembers it.
    fn tree(self: &Rc<Self>, layout: &Layout, path: &mut Vec<bool>) -> gtk::Widget {
        match layout {
            Layout::Leaf(id) => self.pane(*id).map_or_else(
                || gtk::Box::new(gtk::Orientation::Vertical, 0).upcast(),
                |pane| pane.widget.clone().upcast(),
            ),
            Layout::Split {
                side_by_side,
                ratio,
                first,
                second,
            } => {
                let side_by_side = *side_by_side;
                let ratio = i32::from(*ratio);
                let paned = gtk::Paned::new(if side_by_side {
                    gtk::Orientation::Horizontal
                } else {
                    gtk::Orientation::Vertical
                });
                let here = path.clone();
                path.push(false);
                paned.pack1(&self.tree(first, path), true, false);
                path.pop();
                path.push(true);
                paned.pack2(&self.tree(second, path), true, false);
                path.pop();
                // The divider starts where the layout remembers it (the middle for a new split); GTK would size
                // it from the panes' tiny minimum requests. Only moves after that are written back.
                let placed = Rc::new(Cell::new(false));
                let ready = Rc::new(Cell::new(false));
                paned.connect_size_allocate(glib::clone!(@strong placed, @strong ready => move |paned, allocation| {
                    let size = if side_by_side {
                        allocation.width()
                    } else {
                        allocation.height()
                    };
                    if size > 1 && !placed.replace(true) {
                        let weak = paned.downgrade();
                        let ready = ready.clone();
                        glib::idle_add_local_once(move || {
                            if let Some(paned) = weak.upgrade() {
                                paned.set_position(size * ratio / i32::from(RATIO_FULL));
                                ready.set(true);
                            }
                        });
                    }
                }));
                paned.connect_position_notify(glib::clone!(@weak self as app => move |paned| {
                    let size = if side_by_side { paned.allocated_width() } else { paned.allocated_height() };
                    if ready.get() && size > 1 {
                        let ratio = (paned.position() * i32::from(RATIO_FULL) / size).clamp(50, 950);
                        app.divider_moved(&here, ratio as u16);
                    }
                }));
                paned.upcast()
            }
        }
    }
    /// A divider was dragged: remember where it sits.
    fn divider_moved(self: &Rc<Self>, path: &[bool], ratio: u16) {
        let changed = self.panes.layout.borrow_mut().set_ratio(path, ratio);
        if changed {
            self.layout_changed();
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
        // One shown terminal is simply the focused one. With a browser active it is a visible terminal beside the
        // browser, which must stay shown: the browser has no frames to give the daemon.
        if shown.len() < 2 && self.terminal(active).is_some() {
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
