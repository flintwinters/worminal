//! The collapsible side panel: tabs of one-line rows, drawn with the terminal's font and colors like the tab
//! strip and driven from the keyboard. A tab is a `PanelTab`; Bookmarks is the first, and any other list joins
//! by implementing it. The widget exists only while the panel is open and is freed when it closes.
use crate::{
    fonts::Fonts,
    graphics_ffi,
    paint::{self, Metrics, Palette},
    tabs::mix,
};
use gtk::{
    gdk::{self, keys::constants as key, ModifierType as Mod},
    prelude::*,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use worminal_protocol::cell::{COLOR_DEFAULT_BACKGROUND as BG, COLOR_DEFAULT_FOREGROUND as FG};
use worminal_workspace::bookmarks::{Bookmark, Target};

/// The panel's width in terminal cells.
const PANEL_CELLS: i32 = 36;

/// What a panel tab asks the window to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelAction {
    OpenTerminal {
        directory: String,
        master: Option<String>,
    },
    OpenUrl(String),
    RemoveBookmark(Target),
}
/// What the user pressed on a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelKey {
    Open,
    Remove,
}

/// One tab of the panel.
pub trait PanelTab {
    fn title(&self) -> &str;
    /// One line per row.
    fn rows(&self) -> Vec<String>;
    /// Shown instead of rows when there are none.
    fn empty(&self) -> &str;
    fn press(&self, key: PanelKey, row: usize) -> Option<PanelAction>;
}

/// Saved terminal tabs and web pages.
pub struct BookmarksTab {
    items: Vec<Bookmark>,
    home: Option<String>,
}
impl BookmarksTab {
    pub fn new(items: Vec<Bookmark>, home: Option<String>) -> Self {
        Self { items, home }
    }
}
/// `~` for the user's home, so local rows stay short.
fn tilde(path: &str, home: Option<&str>) -> String {
    match home.filter(|home| !home.is_empty() && *home != "/") {
        Some(home) if path == home => "~".into(),
        Some(home) => path
            .strip_prefix(home)
            .filter(|rest| rest.starts_with('/'))
            .map_or_else(|| path.to_owned(), |rest| format!("~{rest}")),
        None => path.to_owned(),
    }
}
fn row_label(bookmark: &Bookmark, home: Option<&str>) -> String {
    match &bookmark.target {
        Target::Terminal {
            directory,
            master: Some(master),
        } => format!("{master}:{directory}"),
        Target::Terminal { directory, .. } => tilde(directory, home),
        Target::Web { url } => bookmark.title.clone().unwrap_or_else(|| url.clone()),
    }
}
impl PanelTab for BookmarksTab {
    fn title(&self) -> &str {
        "Bookmarks"
    }
    fn rows(&self) -> Vec<String> {
        self.items
            .iter()
            .map(|bookmark| row_label(bookmark, self.home.as_deref()))
            .collect()
    }
    fn empty(&self) -> &str {
        "No bookmarks yet. Ctrl+Shift+D bookmarks the current tab."
    }
    fn press(&self, key: PanelKey, row: usize) -> Option<PanelAction> {
        let bookmark = self.items.get(row)?;
        Some(match (key, &bookmark.target) {
            (PanelKey::Remove, target) => PanelAction::RemoveBookmark(target.clone()),
            (PanelKey::Open, Target::Terminal { directory, master }) => PanelAction::OpenTerminal {
                directory: directory.clone(),
                master: master.clone(),
            },
            (PanelKey::Open, Target::Web { url }) => PanelAction::OpenUrl(url.clone()),
        })
    }
}

/// The selected row and the first visible one, independent of GTK.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub selected: usize,
    pub top: usize,
}
impl Cursor {
    /// Keep the selection inside `count` rows and on screen among `visible` ones.
    pub fn clamp(&mut self, count: usize, visible: usize) {
        if count == 0 {
            *self = Self::default();
            return;
        }
        self.selected = self.selected.min(count - 1);
        let visible = visible.max(1);
        if self.selected < self.top {
            self.top = self.selected;
        } else if self.selected >= self.top + visible {
            self.top = self.selected + 1 - visible;
        }
        self.top = self.top.min(count.saturating_sub(visible));
    }
    pub fn step(&mut self, delta: isize, count: usize, visible: usize) {
        self.selected = self.selected.saturating_add_signed(delta);
        self.clamp(count, visible);
    }
    pub fn jump(&mut self, end: bool, count: usize, visible: usize) {
        self.selected = if end { count.saturating_sub(1) } else { 0 };
        self.clamp(count, visible);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    Move(isize),
    Page(isize),
    Edge(bool),
    Tab(isize),
    Open,
    Remove,
    Close,
}
/// The panel's keys. Ctrl and Alt combinations belong to the window and the applications, not to the list.
fn command(value: gdk::keys::Key, state: Mod) -> Option<Command> {
    if state.intersects(Mod::CONTROL_MASK | Mod::MOD1_MASK) {
        return None;
    }
    Some(match value {
        key::Up | key::k => Command::Move(-1),
        key::Down | key::j => Command::Move(1),
        key::Page_Up => Command::Page(-1),
        key::Page_Down => Command::Page(1),
        key::Home | key::g => Command::Edge(false),
        key::End | key::G => Command::Edge(true),
        key::Left | key::h | key::ISO_Left_Tab => Command::Tab(-1),
        key::Right | key::l | key::Tab => Command::Tab(1),
        key::Return | key::KP_Enter | key::space => Command::Open,
        key::Delete | key::BackSpace | key::x => Command::Remove,
        key::Escape | key::q => Command::Close,
        _ => return None,
    })
}

/// How the panel reaches the window.
pub struct Callbacks {
    pub act: Box<dyn Fn(PanelAction)>,
    pub close: Box<dyn Fn()>,
}

struct View {
    widget: gtk::DrawingArea,
    fonts: Rc<RefCell<Fonts>>,
    palette: Rc<RefCell<Palette>>,
    layout: pango::Layout,
    tabs: RefCell<Vec<Box<dyn PanelTab>>>,
    rows: RefCell<Vec<String>>,
    active: Cell<usize>,
    cursor: RefCell<Cursor>,
    callbacks: Rc<Callbacks>,
}

fn header_height(metrics: Metrics) -> i32 {
    metrics.height + 2
}
fn rows_top(metrics: Metrics) -> i32 {
    header_height(metrics) + 2
}
/// Rows that fit between the header and the one-line footer.
fn visible_rows(height: i32, metrics: Metrics) -> usize {
    usize::try_from((height - rows_top(metrics) - metrics.height).max(0) / metrics.height.max(1))
        .unwrap_or(0)
}
/// The row under a press `y` pixels from the top, among `count` rows starting at `top`.
fn row_at(y: i32, metrics: Metrics, top: usize, count: usize) -> Option<usize> {
    let offset = y - rows_top(metrics);
    (offset >= 0)
        .then(|| top + usize::try_from(offset / metrics.height.max(1)).unwrap_or(0))
        .filter(|row| *row < count)
}

impl View {
    fn metrics(&self) -> Metrics {
        self.fonts.borrow_mut().resolve(&self.layout).metrics
    }
    fn plain(&self) {
        self.layout.set_width(-1);
        self.layout.set_ellipsize(pango::EllipsizeMode::None);
        self.layout.set_wrap(pango::WrapMode::Word);
    }
    fn visible(&self) -> usize {
        visible_rows(self.widget.allocated_height(), self.metrics())
    }
    /// Draw `text` with its top-left at `(x, top)`, centered in a line of the font's height.
    fn show(&self, cr: &cairo::Context, text: &str, x: i32, top: i32, metrics: Metrics) {
        self.layout.set_text(text);
        let baseline = top + (metrics.height - metrics.ink_height) / 2 + metrics.ascent;
        cr.move_to(
            f64::from(x),
            f64::from(baseline) - f64::from(self.layout.baseline()) / f64::from(pango::SCALE),
        );
        graphics_ffi::show_layout(cr, &self.layout);
    }
    fn draw(&self, cr: &cairo::Context, width: i32, height: i32) -> Result<(), cairo::Error> {
        let metrics = self.metrics();
        let (background, foreground) = {
            let palette = self.palette.borrow();
            (palette.color(BG), palette.color(FG))
        };
        paint::source(cr, &background);
        cr.paint()?;
        // The panel's own edge, in the same faint line the split panes use.
        paint::source(cr, &mix(&background, &foreground, 0.3));
        cr.rectangle(f64::from(width - 1), 0.0, 1.0, f64::from(height));
        cr.fill()?;
        self.plain();
        let tabs = self.tabs.borrow();
        let mut x = 2;
        for (index, tab) in tabs.iter().enumerate() {
            self.layout.set_text(tab.title());
            let wide = self.layout.pixel_size().0 + 2 * metrics.width;
            let selected = index == self.active.get();
            if selected {
                paint::source(cr, &foreground);
                cr.rectangle(
                    f64::from(x),
                    0.0,
                    f64::from(wide),
                    f64::from(metrics.height),
                );
                cr.fill()?;
            }
            paint::source(cr, if selected { &background } else { &foreground });
            self.show(cr, tab.title(), x + metrics.width, 0, metrics);
            x += wide;
        }
        let text_width = (width - 2 * metrics.width - 1).max(metrics.width);
        let rows = self.rows.borrow();
        if rows.is_empty() {
            paint::source(cr, &mix(&background, &foreground, 0.6));
            self.layout.set_width(text_width * pango::SCALE);
            self.layout.set_wrap(pango::WrapMode::WordChar);
            let empty = tabs.get(self.active.get()).map_or("", |tab| tab.empty());
            self.show(cr, empty, metrics.width, rows_top(metrics), metrics);
            self.plain();
        }
        let cursor = self.cursor.borrow();
        let focused = self.widget.has_focus();
        for (n, row) in rows
            .iter()
            .enumerate()
            .skip(cursor.top)
            .take(visible_rows(height, metrics))
        {
            let top =
                rows_top(metrics) + i32::try_from(n - cursor.top).unwrap_or(0) * metrics.height;
            if n == cursor.selected {
                paint::source(
                    cr,
                    &mix(&background, &foreground, if focused { 0.35 } else { 0.18 }),
                );
                cr.rectangle(
                    1.0,
                    f64::from(top),
                    f64::from(width - 2),
                    f64::from(metrics.height),
                );
                cr.fill()?;
            }
            paint::source(cr, &foreground);
            self.layout.set_width(text_width * pango::SCALE);
            self.layout.set_ellipsize(pango::EllipsizeMode::Middle);
            self.show(cr, row, metrics.width, top, metrics);
            self.plain();
        }
        paint::source(cr, &mix(&background, &foreground, 0.5));
        self.show(
            cr,
            "Enter open  Del remove  Esc close",
            metrics.width,
            height - metrics.height,
            metrics,
        );
        Ok(())
    }
    /// Show the rows of the active tab and keep the selection on a real, visible row.
    fn reload(&self) {
        let rows = self
            .tabs
            .borrow()
            .get(self.active.get())
            .map(|tab| tab.rows())
            .unwrap_or_default();
        let count = rows.len();
        *self.rows.borrow_mut() = rows;
        let visible = self.visible();
        self.cursor.borrow_mut().clamp(count, visible);
        self.widget.queue_draw();
    }
    fn press(&self, key: PanelKey) {
        let action = {
            let tabs = self.tabs.borrow();
            let selected = self.cursor.borrow().selected;
            tabs.get(self.active.get())
                .and_then(|tab| tab.press(key, selected))
        };
        // Callbacks run with no panel state borrowed: they can change the lists and close the panel.
        if let Some(action) = action {
            (self.callbacks.act)(action);
        }
    }
    fn switch(&self, delta: isize) {
        let count = self.tabs.borrow().len();
        if count == 0 {
            return;
        }
        let next = (self.active.get() as isize + delta).rem_euclid(count as isize) as usize;
        self.active.set(next);
        *self.cursor.borrow_mut() = Cursor::default();
        self.reload();
    }
    fn key(&self, event: &gdk::EventKey) -> bool {
        let Some(command) = command(event.keyval(), event.state()) else {
            return false;
        };
        let (count, visible) = (self.rows.borrow().len(), self.visible());
        match command {
            Command::Move(delta) => self.cursor.borrow_mut().step(delta, count, visible),
            Command::Page(delta) => {
                self.cursor.borrow_mut().step(
                    delta * isize::try_from(visible.max(1)).unwrap_or(1),
                    count,
                    visible,
                );
            }
            Command::Edge(end) => self.cursor.borrow_mut().jump(end, count, visible),
            Command::Tab(delta) => self.switch(delta),
            Command::Open => self.press(PanelKey::Open),
            Command::Remove => self.press(PanelKey::Remove),
            Command::Close => (self.callbacks.close)(),
        }
        self.widget.queue_draw();
        true
    }
    /// A click on a tab title switches to it; on a row, opens it.
    fn click(&self, x: f64, y: f64) {
        self.widget.grab_focus();
        let metrics = self.metrics();
        if (y as i32) < header_height(metrics) {
            self.plain();
            let mut left = 2;
            let target = {
                let tabs = self.tabs.borrow();
                tabs.iter().position(|tab| {
                    self.layout.set_text(tab.title());
                    let wide = self.layout.pixel_size().0 + 2 * metrics.width;
                    left += wide;
                    x < f64::from(left)
                })
            };
            if let Some(index) = target.filter(|index| *index != self.active.get()) {
                self.active.set(index);
                *self.cursor.borrow_mut() = Cursor::default();
                self.reload();
            }
            return;
        }
        let row = {
            let cursor = self.cursor.borrow();
            row_at(y as i32, metrics, cursor.top, self.rows.borrow().len())
        };
        if let Some(row) = row {
            self.cursor.borrow_mut().selected = row;
            self.widget.queue_draw();
            self.press(PanelKey::Open);
        }
    }
}

/// The panel's place in the window: an empty slot beside the panes, filled while the panel is open.
pub struct SidePanel {
    slot: gtk::Box,
    view: RefCell<Option<Rc<View>>>,
}
impl Default for SidePanel {
    fn default() -> Self {
        Self::new()
    }
}
impl SidePanel {
    pub fn new() -> Self {
        let slot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        slot.set_no_show_all(true);
        Self {
            slot,
            view: RefCell::new(None),
        }
    }
    pub fn widget(&self) -> &gtk::Box {
        &self.slot
    }
    pub fn is_open(&self) -> bool {
        self.view.borrow().is_some()
    }
    pub fn has_focus(&self) -> bool {
        self.view
            .borrow()
            .as_ref()
            .is_some_and(|view| view.widget.has_focus())
    }
    pub fn focus(&self) {
        if let Some(view) = self.view.borrow().as_ref() {
            view.widget.grab_focus();
        }
    }
    /// Build the panel and give it the keyboard.
    pub fn open(
        &self,
        fonts: Rc<RefCell<Fonts>>,
        palette: Rc<RefCell<Palette>>,
        tabs: Vec<Box<dyn PanelTab>>,
        callbacks: Callbacks,
    ) {
        if self.is_open() {
            return;
        }
        let widget = gtk::DrawingArea::new();
        widget.set_can_focus(true);
        widget.add_events(gdk::EventMask::BUTTON_PRESS_MASK);
        let layout = widget.create_pango_layout(None);
        let view = Rc::new(View {
            widget,
            fonts,
            palette,
            layout,
            tabs: RefCell::new(tabs),
            rows: RefCell::new(Vec::new()),
            active: Cell::new(0),
            cursor: RefCell::new(Cursor::default()),
            callbacks: Rc::new(callbacks),
        });
        view.widget
            .set_size_request(PANEL_CELLS * view.metrics().width, -1);
        view.widget.connect_draw(
            glib::clone!(@weak view => @default-return glib::Propagation::Proceed,
                move |widget, cr| {
                if let Err(error) = view.draw(cr, widget.allocated_width(), widget.allocated_height()) {
                    eprintln!("worminal: side panel: {error}");
                }
                glib::Propagation::Stop
            }),
        );
        view.widget.connect_key_press_event(
            glib::clone!(@weak view => @default-return glib::Propagation::Proceed,
                move |_, event| view.key(event).into()),
        );
        view.widget.connect_button_press_event(
            glib::clone!(@weak view => @default-return glib::Propagation::Proceed,
                move |_, event| {
                // A quick second click arrives as a press and a double-click event; only the press acts.
                if event.button() == 1 && event.event_type() == gdk::EventType::ButtonPress {
                    let (x, y) = event.position();
                    view.click(x, y);
                }
                glib::Propagation::Stop
            }),
        );
        for focus in [true, false] {
            let handler = glib::clone!(@weak view => @default-return glib::Propagation::Proceed,
            move |_: &gtk::DrawingArea, _: &gdk::EventFocus| {
                view.widget.queue_draw();
                glib::Propagation::Proceed
            });
            if focus {
                view.widget.connect_focus_in_event(handler);
            } else {
                view.widget.connect_focus_out_event(handler);
            }
        }
        self.slot.pack_start(&view.widget, false, false, 0);
        self.slot.show();
        view.widget.show();
        view.reload();
        view.widget.grab_focus();
        *self.view.borrow_mut() = Some(view);
    }
    /// Free the panel's widget; only the empty slot remains.
    pub fn close(&self) {
        if let Some(view) = self.view.borrow_mut().take() {
            self.slot.remove(&view.widget);
            self.slot.hide();
        }
    }
    /// The lists changed: show the new rows, keeping the active tab and selection where they still exist.
    pub fn set_tabs(&self, tabs: Vec<Box<dyn PanelTab>>) {
        let view = self.view.borrow().clone();
        if let Some(view) = view {
            let last = tabs.len().saturating_sub(1);
            *view.tabs.borrow_mut() = tabs;
            view.active.set(view.active.get().min(last));
            view.reload();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk::gdk::keys::constants as key;

    fn terminal(directory: &str, master: Option<&str>) -> Bookmark {
        Bookmark {
            target: Target::Terminal {
                directory: directory.into(),
                master: master.map(Into::into),
            },
            title: None,
        }
    }
    fn web(url: &str, title: Option<&str>) -> Bookmark {
        Bookmark {
            target: Target::Web { url: url.into() },
            title: title.map(Into::into),
        }
    }
    const METRICS: Metrics = Metrics {
        width: 8,
        height: 16,
        ink_height: 12,
        ascent: 12,
    };

    #[test]
    fn rows_abbreviate_home_and_name_the_host_and_page() {
        let tab = BookmarksTab::new(
            vec![
                terminal("/home/me", None),
                terminal("/home/me/work", None),
                terminal("/home/mellow", None),
                terminal("/srv/app", Some("me@build")),
                web("https://example.org", Some("Example")),
                web("https://example.org/x", None),
            ],
            Some("/home/me".into()),
        );
        assert_eq!(
            tab.rows(),
            [
                "~",
                "~/work",
                "/home/mellow",
                "me@build:/srv/app",
                "Example",
                "https://example.org/x"
            ]
        );
    }

    #[test]
    fn pressing_a_bookmark_opens_or_removes_exactly_that_bookmark() {
        let tab = BookmarksTab::new(
            vec![
                terminal("/srv/app", Some("me@build")),
                web("https://example.org", None),
            ],
            None,
        );
        assert_eq!(
            tab.press(PanelKey::Open, 0),
            Some(PanelAction::OpenTerminal {
                directory: "/srv/app".into(),
                master: Some("me@build".into())
            })
        );
        assert_eq!(
            tab.press(PanelKey::Open, 1),
            Some(PanelAction::OpenUrl("https://example.org".into()))
        );
        assert_eq!(
            tab.press(PanelKey::Remove, 1),
            Some(PanelAction::RemoveBookmark(Target::Web {
                url: "https://example.org".into()
            }))
        );
        assert_eq!(tab.press(PanelKey::Open, 2), None);
    }

    #[test]
    fn the_cursor_stays_on_a_real_row_and_scrolls_to_keep_it_visible() {
        let mut cursor = Cursor::default();
        cursor.step(1, 10, 4);
        cursor.step(5, 10, 4);
        assert_eq!(
            cursor,
            Cursor {
                selected: 6,
                top: 3
            }
        );
        cursor.step(-100, 10, 4);
        assert_eq!(
            cursor,
            Cursor {
                selected: 0,
                top: 0
            }
        );
        cursor.jump(true, 10, 4);
        assert_eq!(
            cursor,
            Cursor {
                selected: 9,
                top: 6
            }
        );
        // Rows disappeared: the selection and the scroll follow.
        cursor.clamp(3, 4);
        assert_eq!(
            cursor,
            Cursor {
                selected: 2,
                top: 0
            }
        );
        cursor.clamp(0, 4);
        assert_eq!(cursor, Cursor::default());
        // Nothing to move over.
        cursor.step(1, 0, 4);
        cursor.jump(true, 0, 4);
        assert_eq!(cursor, Cursor::default());
    }

    #[test]
    fn keys_map_to_commands_and_leave_ctrl_and_alt_to_the_window() {
        let plain = Mod::empty();
        assert_eq!(command(key::Down, plain), Some(Command::Move(1)));
        assert_eq!(command(key::k, plain), Some(Command::Move(-1)));
        assert_eq!(command(key::Return, plain), Some(Command::Open));
        assert_eq!(command(key::Delete, plain), Some(Command::Remove));
        assert_eq!(command(key::Escape, plain), Some(Command::Close));
        assert_eq!(command(key::Right, plain), Some(Command::Tab(1)));
        assert_eq!(
            command(key::ISO_Left_Tab, Mod::SHIFT_MASK),
            Some(Command::Tab(-1))
        );
        assert_eq!(command(key::G, Mod::SHIFT_MASK), Some(Command::Edge(true)));
        assert_eq!(command(key::a, plain), None);
        for modifier in [
            Mod::CONTROL_MASK,
            Mod::MOD1_MASK,
            Mod::CONTROL_MASK | Mod::SHIFT_MASK,
        ] {
            assert_eq!(command(key::Down, modifier), None);
            assert_eq!(command(key::Return, modifier), None);
        }
    }

    #[test]
    fn presses_map_to_header_and_rows_by_the_same_geometry_as_drawing() {
        // Header 18 px, then a 2 px gap: rows start at 20 and are 16 px tall.
        assert_eq!(rows_top(METRICS), 20);
        assert_eq!(row_at(19, METRICS, 0, 5), None);
        assert_eq!(row_at(20, METRICS, 0, 5), Some(0));
        assert_eq!(row_at(36, METRICS, 0, 5), Some(1));
        assert_eq!(row_at(36, METRICS, 3, 5), Some(4));
        assert_eq!(row_at(36 + 16, METRICS, 3, 5), None);
        // A 100 px panel holds header, rows and the footer line: (100 - 20 - 16) / 16 rows.
        assert_eq!(visible_rows(100, METRICS), 4);
        assert_eq!(visible_rows(10, METRICS), 0);
    }
}
