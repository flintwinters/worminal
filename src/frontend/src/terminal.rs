//! GTK terminal widget. All host actions run after releasing widget state borrows.
use crate::{
    fonts::Fonts,
    input::{self, Autoscroll, KeyAction, Keyboard, Mouse},
    link_action,
    paint::{Painter, Palette, Style},
    viewport::{Damage, Viewport},
};
use gtk::{gdk, prelude::*};
use std::{cell::RefCell, io, rc::Rc, time::Duration};
use worminal_engine::{input::Encoded, paste_chunks};
use worminal_protocol::cell::*;
use worminal_protocol::input::{MOD_KEYS, MOUSE_NONE};
use worminal_protocol::worp::{
    commands::{CommandKind, Control, ControlAction, MouseAction, SelectionOperation},
    events::{Event, EventKind},
};
use worminal_protocol::{WORKSPACE_MAX_COLS, WORKSPACE_MAX_ROWS};

mod file_drop;
mod scroll;

#[derive(Debug)]
pub enum Action {
    Send(CommandKind),
    /// Input for the application, with the view's encoding of it for daemons that predate
    /// `WIRE_FEATURE_INPUT`; newer daemons encode the command against their live modes.
    Input(CommandKind, Encoded),
    Claim,
    CycleTab(bool),
    Resized(u32, u32),
    Print(Vec<u8>),
    Error(String),
}
struct State {
    viewport: Viewport,
    painter: Option<Painter>,
    im: Option<gtk::IMMulticontext>,
    timer: Option<glib::SourceId>,
    keyboard: Keyboard,
    mouse: Mouse,
    tab_scroll: f64,
    /// Middle-click autoscroll and its frame timer; any later press, key or focus loss ends it.
    autoscroll: Option<(Autoscroll, glib::SourceId)>,
    mode: u32,
    cursor: u32,
    blink: bool,
    selecting: bool,
    /// URL pressed with plain button 1; it opens only if released on the same cell.
    link: Option<(String, i32, i32)>,
    /// Full target shown for an explicit link, whose label may differ from it.
    tooltip: Option<String>,
    /// Pointer cell while inside the terminal, and the underlined link under it.
    pointer: Option<(i32, i32)>,
    hover: Option<std::ops::RangeInclusive<usize>>,
    preedit: String,
    columns: u32,
    rows: u32,
    input_pending: bool,
    selection_pending: bool,
    destroyed: bool,
    all_text: Option<Vec<u8>>,
    all_generation: u32,
    all_pending: bool,
    all_copy: Option<gdk::Atom>,
}
struct Inner {
    widget: gtk::DrawingArea,
    fonts: Rc<RefCell<Fonts>>,
    palette: Rc<RefCell<Palette>>,
    state: RefCell<State>,
    sink: Box<dyn Fn(Action)>,
}
#[derive(Clone)]
pub struct Terminal(Rc<Inner>);
impl Terminal {
    pub fn new(
        columns: u32,
        rows: u32,
        fonts: Rc<RefCell<Fonts>>,
        palette: Rc<RefCell<Palette>>,
        sink: impl Fn(Action) + 'static,
    ) -> io::Result<Self> {
        let inner = Rc::new(Inner {
            widget: gtk::DrawingArea::new(),
            fonts,
            palette,
            sink: Box::new(sink),
            state: RefCell::new(State {
                viewport: Viewport::new(columns, rows)?,
                painter: None,
                im: None,
                timer: None,
                keyboard: Keyboard::default(),
                mouse: Mouse::default(),
                tab_scroll: 0.0,
                autoscroll: None,
                mode: 0,
                cursor: 2,
                blink: false,
                selecting: false,
                link: None,
                tooltip: None,
                pointer: None,
                hover: None,
                preedit: String::new(),
                columns,
                rows,
                input_pending: false,
                selection_pending: false,
                destroyed: false,
                all_text: None,
                all_generation: 0,
                all_pending: false,
                all_copy: None,
            }),
        });
        inner.widget.set_can_focus(true);
        inner.widget.add_events(
            gdk::EventMask::BUTTON_PRESS_MASK
                | gdk::EventMask::BUTTON_RELEASE_MASK
                | gdk::EventMask::POINTER_MOTION_MASK
                | gdk::EventMask::LEAVE_NOTIFY_MASK
                | gdk::EventMask::SCROLL_MASK
                | gdk::EventMask::SMOOTH_SCROLL_MASK
                | gdk::EventMask::FOCUS_CHANGE_MASK,
        );
        inner
            .widget
            .connect_map(glib::clone!(@weak inner => move |_| inner.prepare()));
        inner.widget.connect_draw(
            glib::clone!(@weak inner => @default-return glib::Propagation::Proceed,
            move |_, cr| { inner.draw(cr); glib::Propagation::Proceed }),
        );
        inner
            .widget
            .connect_size_allocate(glib::clone!(@weak inner => move |_, allocation| {
                inner.resize(allocation.width(), allocation.height());
            }));
        inner.widget.connect_key_press_event(
            glib::clone!(@weak inner => @default-return glib::Propagation::Proceed,
            move |_, event| inner.key(event).into()),
        );
        inner.widget.connect_key_release_event(
            glib::clone!(@weak inner => @default-return glib::Propagation::Proceed,
            move |_, event| inner.key(event).into()),
        );
        inner.widget.connect_button_press_event(
            glib::clone!(@weak inner => @default-return glib::Propagation::Proceed,
            move |_, event| { inner.button(event); glib::Propagation::Stop }),
        );
        inner.widget.connect_button_release_event(
            glib::clone!(@weak inner => @default-return glib::Propagation::Proceed,
            move |_, event| { inner.button(event); glib::Propagation::Stop }),
        );
        inner.widget.connect_motion_notify_event(
            glib::clone!(@weak inner => @default-return glib::Propagation::Proceed,
            move |_, event| { inner.motion(event); glib::Propagation::Stop }),
        );
        inner.widget.connect_leave_notify_event(
            glib::clone!(@weak inner => @default-return glib::Propagation::Proceed,
            move |_, _| { inner.state.borrow_mut().pointer = None; inner.hover(); glib::Propagation::Proceed }),
        );
        inner.connect_wheel();
        inner.connect_file_drop();
        inner.widget.connect_focus_in_event(
            glib::clone!(@weak inner => @default-return glib::Propagation::Proceed,
            move |_, _| { inner.focus(true); glib::Propagation::Proceed }),
        );
        inner.widget.connect_focus_out_event(
            glib::clone!(@weak inner => @default-return glib::Propagation::Proceed,
            move |_, _| { inner.focus(false); glib::Propagation::Proceed }),
        );
        inner.widget.connect_destroy(glib::clone!(@weak inner => move |_| {
            inner.clear_all_selection();
            inner.stop_autoscroll();
            let (timer, im) = { let mut state = inner.state.borrow_mut(); state.destroyed = true;
                (state.timer.take(), state.im.take()) };
            if let Some(timer) = timer { timer.remove(); }
            if let Some(im) = im { im.focus_out(); im.set_client_window(None); }
        }));
        Ok(Self(inner))
    }
    pub fn widget(&self) -> &gtk::DrawingArea {
        &self.0.widget
    }
    pub fn metrics(&self) -> Option<crate::paint::Metrics> {
        self.0
            .state
            .borrow()
            .painter
            .as_ref()
            .map(|painter| painter.font().metrics)
    }
    pub fn dimensions(&self) -> (u32, u32) {
        let state = self.0.state.borrow();
        (state.columns, state.rows)
    }
    pub fn live(&self) -> bool {
        self.0.state.borrow().viewport.live()
    }
    pub fn release(&self) {
        self.0.state.borrow_mut().viewport.release();
        self.0.widget.queue_draw();
    }
    pub fn configure(&self, mode: u32, cursor: u32, palette_changed: bool) {
        let damage = {
            let mut state = self.0.state.borrow_mut();
            let changed = state.mode ^ mode;
            let cursor_changed = state.cursor != cursor || changed & MODE_HIDE != 0;
            state.mode = mode;
            state.cursor = cursor;
            Damage {
                full: palette_changed || changed & MODE_REVERSE != 0,
                rows: if cursor_changed {
                    vec![state.viewport.frame().cursor_y as usize]
                } else {
                    Vec::new()
                },
            }
        };
        self.0.damage(&damage);
    }
    pub fn event(&self, event: &Event) -> io::Result<()> {
        if let EventKind::Selection {
            generation,
            done,
            text,
        } = &event.kind
        {
            return self.0.all_event(*generation, *done, text);
        }
        let (damage, immediate, clear_all) = {
            let mut state = self.0.state.borrow_mut();
            if state.destroyed {
                return Ok(());
            }
            let old_alternate = state.viewport.frame().alternate;
            let damage = state.viewport.event(event)?;
            let frame = state.viewport.frame();
            let clear_all = old_alternate != frame.alternate && state.all_text.is_some();
            if !clear_all && state.all_text.is_some() && damage.as_ref().is_some_and(|d| d.full) {
                state.viewport.select_all()?;
            }
            let selection = state.selection_pending
                && frame.columns == state.columns as i32
                && frame.rows == state.rows as i32;
            // Ownership/geometry frames can change notebook allocation. Let GTK lay those out first.
            let immediate = damage
                .as_ref()
                .is_some_and(|damage| damage.changed() && !damage.full)
                && (state.input_pending || selection)
                && self.0.widget.is_mapped();
            if immediate {
                state.input_pending = false;
                if selection {
                    state.selection_pending = false;
                }
            }
            (damage, immediate, clear_all)
        };
        if clear_all {
            self.0.clear_all_selection();
        }
        if let Some(damage) = damage {
            crate::timing("frame");
            self.0.damage(&damage);
            // Text can change under a still pointer.
            self.0.hover();
        }
        if immediate {
            if let Some(window) = self.0.widget.window() {
                crate::graphics_ffi::process_updates(&window);
            }
        }
        Ok(())
    }
    pub fn select(&self) {
        let now = glib::monotonic_time();
        let interval = self
            .0
            .widget
            .frame_clock()
            .map(|clock| clock.refresh_info(now).0)
            .unwrap_or(16667)
            .max(1);
        let allowed = self.0.fonts.borrow_mut().selection_allowance(now, interval);
        let mut state = self.0.state.borrow_mut();
        let frame = state.viewport.frame();
        if state.viewport.live()
            && frame.columns == state.columns as i32
            && frame.rows == state.rows as i32
        {
            state.selection_pending = false;
        } else if allowed {
            state.selection_pending = true;
        }
    }
    pub fn select_all(&self) {
        self.0.clear_all_selection();
        let (damage, generation) = {
            let mut state = self.0.state.borrow_mut();
            state.all_text = Some(Vec::new());
            state.all_pending = true;
            state.selecting = false;
            (state.viewport.select_all(), state.all_generation)
        };
        match damage {
            Ok(damage) => self.0.damage(&damage),
            Err(error) => {
                self.0.error(error);
                return;
            }
        }
        self.0.send(CommandKind::Selection {
            generation,
            operation: SelectionOperation::Start,
        });
    }
    pub fn copy(&self, atom: gdk::Atom) {
        self.0.copy(atom);
    }
    pub fn key(&self, event: &gdk::EventKey) -> bool {
        self.0.key(event)
    }
    pub fn paste(&self, atom: gdk::Atom) {
        self.0.paste(atom);
    }
    pub fn zoom(&self) {
        self.0.prepare();
        let layout = self.0.widget.create_pango_layout(None);
        let font = self.0.fonts.borrow_mut().resolve(&layout);
        if let Some(painter) = &mut self.0.state.borrow_mut().painter {
            painter.set_font(font);
        }
        self.0.resize(
            self.0.widget.allocated_width(),
            self.0.widget.allocated_height(),
        );
        self.0.widget.queue_draw();
    }
}
impl Drop for Inner {
    fn drop(&mut self) {
        let state = self.state.get_mut();
        if let Some(timer) = state.timer.take() {
            timer.remove();
        }
        if let Some((_, timer)) = state.autoscroll.take() {
            timer.remove();
        }
    }
}
impl Inner {
    fn emit(&self, action: Action) {
        let alive = !self.state.borrow().destroyed;
        if alive {
            (self.sink)(action);
        }
    }
    fn error(&self, error: impl std::fmt::Display) {
        self.emit(Action::Error(error.to_string()));
    }
    fn send(&self, kind: CommandKind) {
        self.emit(Action::Send(kind));
    }
    fn prepare(self: &Rc<Self>) {
        if self.state.borrow().painter.is_some() || self.state.borrow().destroyed {
            return;
        }
        let layout = self.widget.create_pango_layout(None);
        let font = self.fonts.borrow_mut().resolve(&layout);
        let im = gtk::IMMulticontext::new();
        im.connect_commit(
            glib::clone!(@weak self as inner => move |_, text| inner.input(text.as_bytes())),
        );
        im.connect_preedit_changed(glib::clone!(@weak self as inner => move |im| {
            let (text, _, _) = im.preedit_string();
            inner.state.borrow_mut().preedit = text.to_string(); inner.widget.queue_draw();
        }));
        {
            let mut state = self.state.borrow_mut();
            state.painter = Some(Painter::new(layout, font));
            state.im = Some(im);
        }
        let weak = Rc::downgrade(self);
        let timer = glib::timeout_add_local(Duration::from_millis(800), move || {
            if let Some(inner) = weak.upgrade() {
                inner.tick();
                glib::ControlFlow::Continue
            } else {
                glib::ControlFlow::Break
            }
        });
        self.state.borrow_mut().timer = Some(timer);
        if self.widget.is_realized() {
            self.resize(
                self.widget.allocated_width(),
                self.widget.allocated_height(),
            );
        }
        crate::timing("font");
    }
    fn resize(&self, width: i32, height: i32) {
        let size = {
            let mut state = self.state.borrow_mut();
            let Some(painter) = &state.painter else {
                return;
            };
            let metrics = painter.font().metrics;
            let columns = ((width - 4) / metrics.width).clamp(1, WORKSPACE_MAX_COLS as i32) as u32;
            let rows = ((height - 4) / metrics.height).clamp(1, WORKSPACE_MAX_ROWS as i32) as u32;
            state.columns = columns;
            state.rows = rows;
            (columns, rows)
        };
        self.emit(Action::Resized(size.0, size.1));
    }
    fn damage(&self, damage: &Damage) {
        if damage.full {
            self.widget.queue_draw();
            return;
        }
        let state = self.state.borrow();
        if let Some(painter) = &state.painter {
            for row in &damage.rows {
                let (x, y, w, h) = painter.damage(*row, self.widget.allocated_width());
                self.widget.queue_draw_area(x, y, w, h);
            }
        }
    }
    fn draw(self: &Rc<Self>, cr: &cairo::Context) {
        self.prepare();
        crate::timing("paint");
        let result = {
            let mut state = self.state.borrow_mut();
            let State {
                viewport,
                painter,
                im,
                mode,
                cursor,
                blink,
                preedit,
                hover,
                ..
            } = &mut *state;
            let Some(painter) = painter else {
                return;
            };
            let result = painter.paint(
                viewport,
                &Style {
                    palette: &self.palette.borrow(),
                    mode: *mode,
                    cursor: *cursor,
                    blink: *blink,
                    focused: self.widget.has_focus(),
                    preedit,
                    link: hover.as_ref(),
                },
                cr,
            );
            let frame = viewport.frame();
            let metrics = painter.font().metrics;
            (
                result,
                im.clone(),
                gdk::Rectangle::new(
                    2 + frame.cursor_x * metrics.width,
                    2 + frame.cursor_y * metrics.height,
                    metrics.width,
                    metrics.height,
                ),
            )
        };
        if let Err(error) = result.0 {
            self.error(error);
        }
        if let Some(im) = result.1 {
            im.set_cursor_location(&result.2);
        }
    }
    fn tick(&self) {
        let damage = {
            let mut state = self.state.borrow_mut();
            state.blink = !state.blink;
            if !self.widget.is_mapped() {
                return;
            }
            let frame = state.viewport.frame();
            let rows = (0..frame.rows as usize)
                .filter(|row| {
                    (*row == frame.cursor_y as usize
                        && frame.scroll == 0
                        && state.viewport.live()
                        && state.mode & MODE_HIDE == 0
                        && state.cursor & 1 != 0)
                        || state.viewport.row_blinks(*row)
                })
                .collect();
            Damage { full: false, rows }
        };
        self.damage(&damage);
    }
    fn input(self: &Rc<Self>, bytes: &[u8]) {
        self.deliver(CommandKind::Input {
            bytes: bytes.to_vec(),
            control: Control::Acquire,
        });
    }
    /// Delivers application input. The view encodes it with the modes it draws only to decide
    /// its own effects: the daemon's live modes may already differ.
    fn deliver(self: &Rc<Self>, kind: CommandKind) {
        let encoded = {
            let state = self.state.borrow();
            let alternate = state.viewport.frame().alternate != 0;
            worminal_engine::input::encode(&kind, state.mode, alternate)
        };
        let Some(encoded) = encoded else {
            return;
        };
        if let Encoded::Input(bytes) = &encoded {
            self.prepare();
            self.clear_all_selection();
            crate::timing("input");
            let (damage, claim) = {
                let mut state = self.state.borrow_mut();
                let damage = state.viewport.clear_selection();
                state.input_pending |= !bytes.is_empty();
                (damage, !state.viewport.live())
            };
            match damage {
                Ok(damage) => self.damage(&damage),
                Err(error) => self.error(error),
            }
            if claim {
                self.emit(Action::Claim);
            }
        }
        self.emit(Action::Input(kind, encoded));
    }
    fn key(self: &Rc<Self>, event: &gdk::EventKey) -> bool {
        self.prepare();
        let im = self.state.borrow().im.clone();
        if event.event_type() == gdk::EventType::KeyRelease {
            return im.is_some_and(|im| im.filter_keypress(event));
        }
        self.stop_autoscroll();
        let action = {
            let mut state = self.state.borrow_mut();
            let mode = state.mode;
            let rows = state.viewport.frame().rows as u32;
            state
                .keyboard
                .press(event.keyval(), event.state(), mode, rows)
        };
        match action {
            KeyAction::Send(kind) => self.deliver(kind),
            KeyAction::PastePrimary => self.paste(gdk::SELECTION_PRIMARY),
            KeyAction::Scroll { up, rows } => self.send(CommandKind::Scroll { up, amount: rows }),
            KeyAction::Control(value) => self.send(CommandKind::Control(match value {
                0 => ControlAction::TogglePrinter,
                1 => ControlAction::PrintScreen,
                _ => ControlAction::Break,
            })),
            KeyAction::PrintSelection => {
                let text = self.state.borrow_mut().viewport.selection_text();
                match text {
                    Ok(text) => self.emit(Action::Print(text)),
                    Err(error) => self.error(error),
                }
            }
            KeyAction::Consumed => {}
            KeyAction::Ime => {
                if im.is_some_and(|im| im.filter_keypress(event)) {
                    return true;
                }
                if let Some(c) = event.keyval().to_unicode() {
                    self.input(c.encode_utf8(&mut [0; 4]).as_bytes());
                } else {
                    return false;
                }
            }
        }
        true
    }
    fn clear_all_selection(&self) {
        let cancel = {
            let mut state = self.state.borrow_mut();
            let cancel = state.all_pending.then_some(state.all_generation);
            state.all_generation = state.all_generation.wrapping_add(1);
            state.all_text = None;
            state.all_pending = false;
            state.all_copy = None;
            cancel
        };
        if let Some(generation) = cancel {
            self.send(CommandKind::Selection {
                generation,
                operation: SelectionOperation::Cancel,
            });
        }
    }
    fn all_event(&self, generation: u32, done: bool, text: &[u8]) -> io::Result<()> {
        let copy = {
            let mut state = self.state.borrow_mut();
            if !state.all_pending || generation != state.all_generation {
                return Ok(());
            }
            state.all_text.as_mut().unwrap().extend_from_slice(text);
            if done {
                state.all_pending = false;
                state.all_copy.take()
            } else {
                None
            }
        };
        if !done {
            self.send(CommandKind::Selection {
                generation,
                operation: SelectionOperation::Next,
            });
        } else {
            self.copy(gdk::SELECTION_PRIMARY);
            if let Some(atom) = copy {
                self.copy(atom);
            }
        }
        Ok(())
    }
    fn copy(&self, atom: gdk::Atom) {
        let text = {
            let mut state = self.state.borrow_mut();
            if state.all_pending {
                state.all_copy = Some(atom);
                return;
            }
            match &state.all_text {
                Some(text) => Ok(text.clone()),
                None => state.viewport.selection_text(),
            }
        };
        match text {
            Ok(text) if !text.is_empty() => {
                gtk::Clipboard::get(&atom).set_text(&String::from_utf8_lossy(&text))
            }
            Err(error) => self.error(error),
            _ => {}
        }
    }
    fn paste(self: &Rc<Self>, atom: gdk::Atom) {
        let weak = Rc::downgrade(self);
        gtk::Clipboard::get(&atom).request_text(move |_, text| {
            let Some(inner) = weak
                .upgrade()
                .filter(|inner| !inner.state.borrow().destroyed)
            else {
                return;
            };
            if let Some(text) = text {
                inner.paste_bytes(text.as_bytes().to_vec());
            }
        });
    }
    fn paste_bytes(self: &Rc<Self>, bytes: Vec<u8>) {
        let chunks = paste_chunks(&bytes);
        let last = chunks.len() - 1;
        for (index, chunk) in chunks.into_iter().enumerate() {
            self.deliver(CommandKind::Paste {
                bytes: chunk.to_vec(),
                begin: index == 0,
                end: index == last,
            });
        }
    }
    fn focus(self: &Rc<Self>, focused: bool) {
        self.prepare();
        if !focused {
            self.stop_autoscroll();
        }
        let im = self.state.borrow().im.clone();
        if let Some(im) = im {
            im.set_client_window(self.widget.window().as_ref());
            if focused {
                im.focus_in();
            } else {
                im.focus_out();
            }
        }
        if self.state.borrow().mode & MODE_FOCUS != 0 {
            self.send(CommandKind::FocusState(focused));
        }
        let row = self.state.borrow().viewport.frame().cursor_y as usize;
        self.damage(&Damage {
            full: false,
            rows: vec![row],
        });
    }
    fn pointer(&self, x: f64, y: f64) -> (i32, i32) {
        let state = self.state.borrow();
        let frame = state.viewport.frame();
        let metrics = state.painter.as_ref().unwrap().font().metrics;
        input::position(
            x,
            y,
            metrics.width,
            metrics.height,
            frame.columns,
            frame.rows,
        )
    }
    fn button(self: &Rc<Self>, event: &gdk::EventButton) {
        self.prepare();
        let release = event.event_type() == gdk::EventType::ButtonRelease;
        let was_scrolling = !release && self.stop_autoscroll();
        let shift = event.state().contains(gdk::ModifierType::SHIFT_MASK);
        if !release && !self.state.borrow().viewport.live() {
            self.emit(Action::Claim);
        }
        self.widget.grab_focus();
        let (px, py) = event.position();
        let (x, y) = self.pointer(px, py);
        if event.button() == 2 && !shift {
            // A press that ended autoscroll must not restart it; its release is ignored too.
            if !(release || was_scrolling) {
                self.start_autoscroll(py);
            }
            return;
        }
        // Links take button 1 before mouse reporting, as in the retired X11 frontend; Shift
        // bypasses them to select or report. Both press and release stay local.
        if event.button() == 1 && !shift {
            let mut state = self.state.borrow_mut();
            if release {
                if let Some((url, column, row)) = state.link.take() {
                    drop(state);
                    if (column, row) == (x, y) {
                        match link_action::resolve(&url) {
                            Some(link_action::Action::Open(url)) => {
                                if let Err(error) = crate::startup::open_url(&url) {
                                    eprintln!("worminal: open {url}: {error}");
                                }
                            }
                            // Keyboard path on purpose: bracketed paste would suppress execution.
                            Some(link_action::Action::Input(bytes)) => self.input(&bytes),
                            None => {}
                        }
                    }
                    return;
                }
            } else {
                if let Some(target) =
                    link_action::target_at(&state.viewport, x as usize, y as usize)
                {
                    state.link = Some((target.uri, x, y));
                    return;
                }
            }
        }
        // An application reporting the mouse owns unshifted buttons, even when the daemon's
        // live modes encode no report for this one.
        let reported = {
            let mut state = self.state.borrow_mut();
            let reported = state.mode & MODE_MOUSE != 0 && !shift;
            if reported {
                state.mouse.button = if release { 0 } else { event.button() };
            }
            reported
        };
        if reported {
            let action = if release {
                MouseAction::Release
            } else {
                MouseAction::Press
            };
            if let Some(button) = event.button().checked_sub(1) {
                self.deliver(mouse(button, (x, y), event.state(), action));
            }
            return;
        }
        if event.button() == 1 {
            if !release {
                self.clear_all_selection();
            }
            let result = {
                let mut state = self.state.borrow_mut();
                state.selecting = !release;
                let snap = match event.event_type() {
                    gdk::EventType::DoubleButtonPress => 1,
                    gdk::EventType::TripleButtonPress => 2,
                    _ => 0,
                };
                state.viewport.select(
                    x,
                    y,
                    !release,
                    snap,
                    event.state().contains(gdk::ModifierType::MOD1_MASK),
                    release,
                )
            };
            match result {
                Ok(damage) => self.damage(&damage),
                Err(error) => self.error(error),
            }
            if release {
                self.copy(gdk::SELECTION_PRIMARY);
            }
        } else if event.button() == 2 && release {
            self.paste(gdk::SELECTION_PRIMARY);
        }
    }
    /// Underline the link under the pointer, redrawing only the rows of a changed span.
    fn hover(&self) {
        let (tooltip, spans, columns) = {
            let mut state = self.state.borrow_mut();
            let columns = state.viewport.frame().columns as usize;
            let target = state
                .pointer
                .and_then(|(x, y)| link_action::target_at(&state.viewport, x as usize, y as usize));
            let tooltip = target
                .as_ref()
                .filter(|target| target.explicit)
                .map(|target| target.uri.clone());
            let new = target.map(|target| target.span);
            let tooltip = (tooltip != state.tooltip).then(|| {
                state.tooltip.clone_from(&tooltip);
                tooltip
            });
            let spans = (new != state.hover)
                .then(|| (std::mem::replace(&mut state.hover, new.clone()), new));
            (tooltip, spans, columns)
        };
        // Set outside the state borrow: GTK property changes can reenter this widget's handlers.
        if let Some(tooltip) = tooltip {
            self.widget.set_tooltip_text(tooltip.as_deref());
        }
        let Some((old, new)) = spans else {
            return;
        };
        let rows = [old, new]
            .into_iter()
            .flatten()
            .flat_map(|span| span.start() / columns..=span.end() / columns)
            .collect();
        self.damage(&Damage { full: false, rows });
    }
    fn motion(self: &Rc<Self>, event: &gdk::EventMotion) {
        self.prepare();
        let (px, py) = event.position();
        let (x, y) = self.pointer(px, py);
        let moved = {
            let mut state = self.state.borrow_mut();
            if let Some((scroll, _)) = &mut state.autoscroll {
                scroll.y = py;
            }
            state.pointer.replace((x, y)) != Some((x, y))
        };
        // Frame commits refresh hover separately; subcell motion cannot change the target.
        if moved {
            self.hover();
        }
        let result = {
            let mut state = self.state.borrow_mut();
            if state.mode & MODE_MOUSE != 0
                && !event.state().contains(gdk::ModifierType::SHIFT_MASK)
            {
                // Motion reports only reach a live view's application.
                let report = (state.mouse.last != (x, y) && state.viewport.live()).then(|| {
                    let button = state.mouse.button.checked_sub(1).unwrap_or(MOUSE_NONE);
                    mouse(button, (x, y), event.state(), MouseAction::Motion)
                });
                state.mouse.last = (x, y);
                (report, None)
            } else if state.selecting {
                (
                    None,
                    Some(state.viewport.select(
                        x,
                        y,
                        false,
                        0,
                        event.state().contains(gdk::ModifierType::MOD1_MASK),
                        false,
                    )),
                )
            } else {
                (None, None)
            }
        };
        if let Some(report) = result.0 {
            self.deliver(report);
        }
        if let Some(result) = result.1 {
            match result {
                Ok(damage) => self.damage(&damage),
                Err(error) => self.error(error),
            }
        }
    }
    fn start_autoscroll(self: &Rc<Self>, y: f64) {
        const FRAME: Duration = Duration::from_millis(16);
        let weak = Rc::downgrade(self);
        let timer = glib::timeout_add_local(FRAME, move || match weak.upgrade() {
            Some(inner) => {
                inner.autoscroll_frame(FRAME.as_secs_f64());
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        self.state.borrow_mut().autoscroll = Some((Autoscroll::new(y), timer));
        self.set_cursor(Some("all-scroll"));
    }
    /// Ends autoscroll, reporting whether it was running.
    fn stop_autoscroll(&self) -> bool {
        let Some((_, timer)) = self.state.borrow_mut().autoscroll.take() else {
            return false;
        };
        timer.remove();
        self.set_cursor(None);
        true
    }
    fn set_cursor(&self, name: Option<&str>) {
        if let Some(window) = self.widget.window() {
            let cursor = name.and_then(|name| gdk::Cursor::from_name(&window.display(), name));
            window.set_cursor(cursor.as_ref());
        }
    }
    fn autoscroll_frame(self: &Rc<Self>, seconds: f64) {
        let step = {
            let mut state = self.state.borrow_mut();
            let height = state
                .painter
                .as_ref()
                .map_or(1.0, |painter| f64::from(painter.font().metrics.height));
            state
                .autoscroll
                .as_mut()
                .and_then(|(scroll, _)| scroll.step(seconds, height))
        };
        if let Some((up, lines)) = step {
            let cell = self.state.borrow().pointer.unwrap_or_default();
            self.scroll(up, lines, lines, cell, gdk::ModifierType::empty());
        }
    }
}

/// A mouse command for cell `(x, y)`; pointer coordinates are clamped to the grid.
fn mouse(
    button: u32,
    (x, y): (i32, i32),
    state: gdk::ModifierType,
    action: MouseAction,
) -> CommandKind {
    CommandKind::Mouse {
        button,
        column: x.max(0) as u32,
        row: y.max(0) as u32,
        modifiers: state.bits() & MOD_KEYS,
        action,
    }
}
