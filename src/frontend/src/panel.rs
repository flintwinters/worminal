//! The collapsible side panel: tabs of one-line rows, drawn with the terminal's font and colors like the tab
//! strip and driven from the keyboard. A tab is a `PanelTab`; Bookmarks is the first, and any other list joins
//! by implementing it. The widget exists only while the panel is open and is freed when it closes.
pub use crate::panel_tabs::{BookmarksTab, PanelAction, PanelKey, PanelTab};
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
use worminal_workspace::bookmarks::Bookmark;

/// The panel's width in terminal cells.
const PANEL_CELLS: i32 = 40;

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
    Add,
    Rename,
    Close,
}
/// What the panel is asking the user to type.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
enum Stage {
    /// A new bookmark's host (no spaces).
    #[default]
    Host,
    /// An optional name for the bookmark just typed; it is saved when this is answered.
    Name(Bookmark),
    /// A new name for the selected row.
    Rename,
}
impl Stage {
    /// Hosts have no spaces; names do.
    fn spaces(&self) -> bool {
        !matches!(self, Self::Host)
    }
}
/// What a key does while the panel is asking the user to type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edit {
    Insert(char),
    Erase,
    Clear,
    Submit,
    Cancel,
}
/// Typing keeps every plain key as text (so `j`, `x` and `a` are letters here); Ctrl+U clears, and other Ctrl and
/// Alt combinations stay with the window. Spaces are text only where `spaces` says so (names, not hosts).
fn edit(value: gdk::keys::Key, state: Mod, spaces: bool) -> Option<Edit> {
    if state.contains(Mod::CONTROL_MASK) {
        return (value == key::u && !state.contains(Mod::MOD1_MASK)).then_some(Edit::Clear);
    }
    if state.contains(Mod::MOD1_MASK) {
        return None;
    }
    match value {
        key::Return | key::KP_Enter => Some(Edit::Submit),
        key::Escape => Some(Edit::Cancel),
        key::BackSpace => Some(Edit::Erase),
        _ => value
            .to_unicode()
            .filter(|c| !c.is_control() && (spaces || !c.is_whitespace()))
            .map(Edit::Insert),
    }
}
/// The bookmark with the typed name (blank: none), or why that name is refused.
fn name_it(bookmark: &Bookmark, text: &str) -> Result<PanelAction, String> {
    let named = Bookmark {
        title: Some(text.trim().to_owned()).filter(|title| !title.is_empty()),
        ..bookmark.clone()
    };
    named
        .validate()
        .map_err(|_| "That name is too long.".to_owned())?;
    Ok(PanelAction::AddBookmark(named))
}
/// Longest text the prompt keeps; hosts are far shorter.
const PROMPT_LIMIT: usize = 255;
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
        key::a => Command::Add,
        key::r => Command::Rename,
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
    /// The labelled line above the row that starts a second group (`PanelTab::separator`).
    separator: RefCell<Option<(usize, String)>>,
    active: Cell<usize>,
    cursor: RefCell<Cursor>,
    callbacks: Rc<Callbacks>,
    /// The text being typed for the active tab's `add`, while the panel is asking.
    prompt: RefCell<Option<String>>,
    /// Which question the prompt is answering.
    stage: RefCell<Stage>,
    /// Why the last submission was refused; it replaces the footer until the next key.
    notice: RefCell<Option<String>>,
}

/// ANSI red, for a refused entry.
fn palette_red(palette: &Rc<RefCell<Palette>>) -> gdk::RGBA {
    palette.borrow().color(1)
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
/// A separator is a line of its own above row `split`, so lines and rows differ by one after it.
fn line_of(row: usize, split: Option<usize>) -> usize {
    row + usize::from(split.is_some_and(|split| row >= split))
}
/// The row on a line, or `None` for the separator's own line.
fn row_on(line: usize, split: Option<usize>) -> Option<usize> {
    match split {
        Some(split) if line == split => None,
        Some(split) if line > split => Some(line - 1),
        _ => Some(line),
    }
}
/// The line drawn first when row `top` is the first one scrolled to: the separator shows above its row.
fn first_line(top: usize, split: Option<usize>) -> usize {
    line_of(top, split) - usize::from(split == Some(top))
}
/// The row under a press `y` pixels from the top, among `count` rows starting at `top`; none on the separator.
fn row_at(
    y: i32,
    metrics: Metrics,
    top: usize,
    count: usize,
    split: Option<usize>,
) -> Option<usize> {
    let offset = y - rows_top(metrics);
    if offset < 0 {
        return None;
    }
    let line =
        first_line(top, split) + usize::try_from(offset / metrics.height.max(1)).unwrap_or(0);
    row_on(line, split).filter(|row| *row < count)
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
    /// Rows that stay fully on screen; a separator may take one of the lines.
    fn visible(&self) -> usize {
        let lines = visible_rows(self.widget.allocated_height(), self.metrics());
        if self.separator.borrow().is_some() {
            lines.saturating_sub(1).max(1)
        } else {
            lines
        }
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
        let split = self.separator.borrow().as_ref().map(|(row, _)| *row);
        let start = first_line(cursor.top, split);
        for line in start..start + visible_rows(height, metrics) {
            let top = rows_top(metrics) + i32::try_from(line - start).unwrap_or(0) * metrics.height;
            let Some(n) = row_on(line, split) else {
                // The separator: its label, then a faint rule to the panel's edge.
                let label = self
                    .separator
                    .borrow()
                    .as_ref()
                    .map(|(_, label)| label.clone());
                let label = label.unwrap_or_default();
                paint::source(cr, &mix(&background, &foreground, 0.6));
                self.show(cr, &label, metrics.width, top, metrics);
                self.layout.set_text(&label);
                let after = metrics.width + self.layout.pixel_size().0 + metrics.width / 2;
                paint::source(cr, &mix(&background, &foreground, 0.3));
                cr.rectangle(
                    f64::from(after),
                    f64::from(top + metrics.height / 2),
                    f64::from((width - 1 - after - metrics.width).max(0)),
                    1.0,
                );
                cr.fill()?;
                continue;
            };
            let Some(row) = rows.get(n) else { break };
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
        self.draw_footer(cr, width, height, metrics)?;
        Ok(())
    }
    /// The last line: the hint, the question while typing, or why the last answer was refused.
    fn draw_footer(
        &self,
        cr: &cairo::Context,
        width: i32,
        height: i32,
        metrics: Metrics,
    ) -> Result<(), cairo::Error> {
        let (background, foreground) = {
            let palette = self.palette.borrow();
            (palette.color(BG), palette.color(FG))
        };
        let text_width = (width - 2 * metrics.width - 1).max(metrics.width);
        let tabs = self.tabs.borrow();
        let footer = height - metrics.height;
        let prompting = self.prompt.borrow().clone();
        let notice = self.notice.borrow().clone();
        if let Some(notice) = notice {
            paint::source(cr, &palette_red(&self.palette));
            self.show(cr, &notice, metrics.width, footer, metrics);
        } else if let Some(text) = prompting {
            let label = match &*self.stage.borrow() {
                Stage::Host => tabs
                    .get(self.active.get())
                    .and_then(|tab| tab.add_prompt())
                    .unwrap_or("Add")
                    .to_owned(),
                Stage::Name(_) => "Name (optional)".to_owned(),
                Stage::Rename => "Name".to_owned(),
            };
            paint::source(cr, &foreground);
            // The end of a long entry is the part being typed.
            self.layout.set_width(text_width * pango::SCALE);
            self.layout.set_ellipsize(pango::EllipsizeMode::Start);
            self.show(
                cr,
                &format!("{label}: {text}_"),
                metrics.width,
                footer,
                metrics,
            );
            self.plain();
        } else {
            paint::source(cr, &mix(&background, &foreground, 0.5));
            self.show(
                cr,
                "Enter open  a add  r name  Del remove",
                metrics.width,
                footer,
                metrics,
            );
        }
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
        *self.separator.borrow_mut() = self
            .tabs
            .borrow()
            .get(self.active.get())
            .and_then(|tab| tab.separator())
            .map(|(row, label)| (row, label.to_owned()));
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
        self.prompt.borrow_mut().take();
        *self.cursor.borrow_mut() = Cursor::default();
        self.reload();
    }
    fn has_focus_and_paste(self: &Rc<Self>) -> bool {
        self.widget.has_focus() && self.paste()
    }
    /// Keys while typing: edit the text, send it on Enter, leave on Escape.
    fn type_key(&self, event: &gdk::EventKey) -> bool {
        let spaces = self.stage.borrow().spaces();
        let Some(edit) = edit(event.keyval(), event.state(), spaces) else {
            return false;
        };
        self.notice.borrow_mut().take();
        match edit {
            Edit::Insert(c) => {
                if let Some(text) = self.prompt.borrow_mut().as_mut() {
                    if text.len() < PROMPT_LIMIT {
                        text.push(c);
                    }
                }
            }
            Edit::Erase => {
                if let Some(text) = self.prompt.borrow_mut().as_mut() {
                    text.pop();
                }
            }
            Edit::Clear => {
                if let Some(text) = self.prompt.borrow_mut().as_mut() {
                    text.clear();
                }
            }
            Edit::Cancel => {
                self.prompt.borrow_mut().take();
                self.stage.take();
            }
            Edit::Submit => self.submit(),
        }
        self.widget.queue_draw();
        true
    }
    /// Send what was typed to the question being asked. A refusal stays in the footer with the text kept for
    /// fixing; a host that was accepted is followed by the question of what to call it.
    fn submit(&self) {
        let text = self.prompt.borrow().clone().unwrap_or_default();
        let stage = self.stage.borrow().clone();
        let selected = self.cursor.borrow().selected;
        let result = {
            let tabs = self.tabs.borrow();
            let tab = tabs.get(self.active.get());
            match &stage {
                Stage::Host => tab.map(|tab| tab.add(&text)),
                Stage::Name(bookmark) => Some(name_it(bookmark, &text)),
                Stage::Rename => tab.map(|tab| tab.rename(selected, &text)),
            }
        };
        match (result, stage) {
            // A host was accepted: ask what to call it before saving.
            (Some(Ok(PanelAction::AddBookmark(bookmark))), Stage::Host) => {
                *self.stage.borrow_mut() = Stage::Name(bookmark);
                *self.prompt.borrow_mut() = Some(String::new());
            }
            (Some(Ok(action)), _) => {
                self.prompt.borrow_mut().take();
                self.stage.take();
                // Callbacks run with no panel state borrowed.
                (self.callbacks.act)(action);
            }
            (Some(Err(why)), _) => *self.notice.borrow_mut() = Some(why),
            (None, _) => {
                self.prompt.borrow_mut().take();
                self.stage.take();
            }
        }
    }
    /// Start typing a new bookmark's host if the active tab can add something.
    fn begin_add(&self) {
        let can = self
            .tabs
            .borrow()
            .get(self.active.get())
            .is_some_and(|tab| tab.add_prompt().is_some());
        if can {
            *self.prompt.borrow_mut() = Some(String::new());
            *self.stage.borrow_mut() = Stage::Host;
            self.notice.borrow_mut().take();
        }
    }
    /// Start naming the selected row, from the name it has now.
    fn begin_rename(&self) {
        let selected = self.cursor.borrow().selected;
        let current = self
            .tabs
            .borrow()
            .get(self.active.get())
            .and_then(|tab| tab.name_of(selected));
        if let Some(current) = current {
            *self.prompt.borrow_mut() = Some(current);
            *self.stage.borrow_mut() = Stage::Rename;
            self.notice.borrow_mut().take();
        }
    }
    /// Paste the clipboard's first line into the prompt; false when the panel is not asking for anything.
    fn paste(self: &Rc<Self>) -> bool {
        if self.prompt.borrow().is_none() {
            return false;
        }
        let view = Rc::downgrade(self);
        gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD).request_text(move |_, text| {
            let (Some(view), Some(text)) = (view.upgrade(), text) else {
                return;
            };
            let line = text.lines().next().unwrap_or_default();
            if let Some(typed) = view.prompt.borrow_mut().as_mut() {
                let spaces = view.stage.borrow().spaces();
                for c in line
                    .trim()
                    .chars()
                    .filter(|c| !c.is_control() && (spaces || !c.is_whitespace()))
                {
                    if typed.len() < PROMPT_LIMIT {
                        typed.push(c);
                    }
                }
            }
            view.notice.borrow_mut().take();
            view.widget.queue_draw();
        });
        true
    }
    fn key(&self, event: &gdk::EventKey) -> bool {
        if self.prompt.borrow().is_some() {
            return self.type_key(event);
        }
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
            Command::Add => self.begin_add(),
            Command::Rename => self.begin_rename(),
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
            let split = self.separator.borrow().as_ref().map(|(row, _)| *row);
            row_at(
                y as i32,
                metrics,
                cursor.top,
                self.rows.borrow().len(),
                split,
            )
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
    /// Route a paste into the panel's prompt; false when the panel is not asking for anything.
    pub fn paste(&self) -> bool {
        let view = self.view.borrow().clone();
        view.is_some_and(|view| view.has_focus_and_paste())
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
            separator: RefCell::new(None),
            active: Cell::new(0),
            cursor: RefCell::new(Cursor::default()),
            callbacks: Rc::new(callbacks),
            prompt: RefCell::new(None),
            stage: RefCell::new(Stage::default()),
            notice: RefCell::new(None),
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
    use worminal_workspace::bookmarks::Target;

    const METRICS: Metrics = Metrics {
        width: 8,
        height: 16,
        ink_height: 12,
        ascent: 12,
    };

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
        assert_eq!(command(key::z, plain), None);
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
        assert_eq!(row_at(19, METRICS, 0, 5, None), None);
        assert_eq!(row_at(20, METRICS, 0, 5, None), Some(0));
        assert_eq!(row_at(36, METRICS, 0, 5, None), Some(1));
        assert_eq!(row_at(36, METRICS, 3, 5, None), Some(4));
        assert_eq!(row_at(36 + 16, METRICS, 3, 5, None), None);
        // A 100 px panel holds header, rows and the footer line: (100 - 20 - 16) / 16 rows.
        assert_eq!(visible_rows(100, METRICS), 4);
        assert_eq!(visible_rows(10, METRICS), 0);
    }

    #[test]
    fn a_separator_is_a_line_of_its_own_that_no_press_can_select() {
        // Rows 0 and 1, the separator line, then rows 2 and 3 (lines 0, 1, 2, 3, 4).
        let split = Some(2);
        let at = |line: i32, top| row_at(20 + 16 * line, METRICS, top, 4, split);
        assert_eq!(
            [at(0, 0), at(1, 0), at(2, 0), at(3, 0), at(4, 0)],
            [Some(0), Some(1), None, Some(2), Some(3)]
        );
        // Scrolled so the first remote row is first: its separator shows above it.
        assert_eq!([at(0, 2), at(1, 2), at(2, 2)], [None, Some(2), Some(3)]);
        // Scrolled past it: plain rows again.
        assert_eq!([at(0, 3), at(1, 3)], [Some(3), None]);
        assert_eq!(
            [line_of(1, split), line_of(2, split), line_of(3, split)],
            [1, 3, 4]
        );
        assert_eq!(
            [row_on(1, split), row_on(2, split), row_on(3, split)],
            [Some(1), None, Some(2)]
        );
        assert_eq!(
            (
                first_line(2, split),
                first_line(1, split),
                first_line(0, None)
            ),
            (2, 1, 0)
        );
    }

    #[test]
    fn while_typing_every_plain_key_is_text_and_ctrl_alt_stay_with_the_window() {
        let plain = Mod::empty();
        assert_eq!(command(key::a, plain), Some(Command::Add));
        // The list's own keys are letters in a host name.
        for letter in [
            key::a,
            key::j,
            key::k,
            key::x,
            key::q,
            key::g,
            key::l,
            key::h,
        ] {
            let c = letter.to_unicode().unwrap();
            assert_eq!(edit(letter, plain, false), Some(Edit::Insert(c)));
        }
        assert_eq!(
            edit(key::G, Mod::SHIFT_MASK, false),
            Some(Edit::Insert('G'))
        );
        assert_eq!(
            edit(key::at, Mod::SHIFT_MASK, false),
            Some(Edit::Insert('@'))
        );
        assert_eq!(edit(key::_1, plain, false), Some(Edit::Insert('1')));
        assert_eq!(edit(key::period, plain, false), Some(Edit::Insert('.')));
        assert_eq!(edit(key::Return, plain, false), Some(Edit::Submit));
        assert_eq!(edit(key::KP_Enter, plain, false), Some(Edit::Submit));
        assert_eq!(edit(key::Escape, plain, false), Some(Edit::Cancel));
        assert_eq!(edit(key::BackSpace, plain, false), Some(Edit::Erase));
        assert_eq!(edit(key::u, Mod::CONTROL_MASK, false), Some(Edit::Clear));
        // Hosts have no spaces, and arrows or function keys are not text.
        assert_eq!(edit(key::space, plain, false), None);
        assert_eq!(edit(key::Tab, plain, false), None);
        assert_eq!(edit(key::Up, plain, false), None);
        assert_eq!(edit(key::F5, plain, false), None);
        for modifier in [
            Mod::CONTROL_MASK,
            Mod::MOD1_MASK,
            Mod::CONTROL_MASK | Mod::SHIFT_MASK,
        ] {
            assert_eq!(edit(key::a, modifier, false), None);
            assert_eq!(edit(key::Return, modifier, false), None);
        }
    }

    #[test]
    fn names_may_contain_spaces_and_hosts_may_not() {
        let plain = Mod::empty();
        assert_eq!(edit(key::space, plain, true), Some(Edit::Insert(' ')));
        assert_eq!(edit(key::space, plain, false), None);
        assert!(Stage::Name(Bookmark {
            target: Target::Host {
                endpoint: "h".into(),
                port: None
            },
            title: None,
        })
        .spaces());
        assert!(Stage::Rename.spaces());
        assert!(!Stage::Host.spaces());
        assert_eq!(command(key::r, plain), Some(Command::Rename));
    }

    #[test]
    fn a_typed_name_is_kept_trimmed_and_a_blank_one_means_no_name() {
        let host = Bookmark {
            target: Target::Host {
                endpoint: "me@build".into(),
                port: None,
            },
            title: None,
        };
        let named = |text: &str| match name_it(&host, text) {
            Ok(PanelAction::AddBookmark(bookmark)) => bookmark.title,
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(named("  Build box  "), Some("Build box".into()));
        assert_eq!(named(""), None);
        assert_eq!(named("   "), None);
        assert!(name_it(&host, &"x".repeat(300)).is_err());
    }
}
