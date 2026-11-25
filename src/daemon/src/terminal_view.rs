//! Interactive VT presentation of a Worp session, including over an ordinary SSH PTY.
mod input;
mod screen;
mod tabs;
mod tty;

use crate::reactor::{self, Interest};
use std::{
    collections::HashMap,
    io::{self, Read, Write},
    os::{fd::AsRawFd, unix::net::UnixStream},
    path::Path,
    time::{Duration, Instant},
};
use worminal_protocol::{
    input::{MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOUSE_WHEEL_DOWN, MOUSE_WHEEL_UP},
    worp::{
        commands::{Command, CommandKind, Control, MouseAction, OpenMode},
        events::{split_directory_field, Event, EventKind, TabKind, TabRecord},
        launch::Launch,
        session::Session,
        v2_events,
    },
    Decoder, WIRE_FEATURE_ATTENTION, WIRE_FEATURE_HOSTS, WORKSPACE_MAX_ROWS,
};
use worminal_workspace::{
    bookmarks::{Bookmark, Bookmarks, Target},
    tab_controls::{self, Action as TabAction},
    tab_view::Tab,
};

pub(crate) const HELP: &str = "worminald terminal
Open the workspace in a VT-compatible terminal, or run ssh -t USER@HOST worminald terminal.
The Linux host renders the tab bar and shell pane over the terminal. Browser tabs are skipped.
Click to select, drag to reorder, middle-click to close, and scroll the tab bar to cycle.
Ctrl+Tab and Ctrl+Shift+Tab cycle when the outer terminal forwards them.
Type Ctrl+] then n/p to switch tabs, t to open a shell, k/j to scroll, or q to detach.
Type Ctrl+] twice to send it to the shell. Detaching keeps the workspace running.
";

type Sender = Session<io::Empty, UnixStream>;
type Size = (u32, u32);

#[derive(Clone, Copy)]
struct Body {
    columns: u32,
    rows: u32,
    top: u32,
}

struct View {
    sender: Sender,
    size: Size,
    active: u32,
    tabs: Vec<TabRecord>,
    tab_bar: tabs::Bar,
    tab_entries: Vec<Tab>,
    bookmarks: Vec<Bookmark>,
    attention: HashMap<u32, u32>,
    palette: Vec<Option<u32>>,
    palette_overrides: Vec<Vec<u8>>,
    order: Vec<u32>,
    pressed_tab: Option<u32>,
    screen: screen::Screen,
    input: input::Input,
    opening: bool,
    received_catalog: bool,
}

impl View {
    fn body_size(&self) -> Body {
        let top = self.tab_bar.rows.max(1);
        Body {
            columns: self.size.0,
            rows: (self.size.1 - top).min(WORKSPACE_MAX_ROWS),
            top,
        }
    }

    fn pty_size(&self) -> Size {
        let body = self.body_size();
        (body.columns, body.rows.max(1))
    }

    fn severity(&self, id: u32) -> u32 {
        self.attention.get(&id).copied().unwrap_or_default()
    }

    fn bookmark_target(tab: &TabRecord) -> Option<Target> {
        let (directory, user) = split_directory_field(&tab.directory);
        let directory = Path::new(std::str::from_utf8(directory).ok()?);
        let host = std::str::from_utf8(&tab.host).ok()?;
        Target::terminal(
            directory,
            user.is_some(),
            (!host.is_empty()).then(|| host.to_owned()),
        )
    }

    fn update_tab_entries(&mut self) {
        let show_empty_stars = worminal_config::get().unbookmarked_stars;
        self.tab_entries = self
            .tabs
            .iter()
            .map(|record| {
                let mut tab = Tab::terminal(record);
                if let Some(target) = Self::bookmark_target(record) {
                    tab.starred = self
                        .bookmarks
                        .iter()
                        .any(|bookmark| bookmark.target == target);
                    tab.starrable = tab.starred || show_empty_stars;
                }
                tab
            })
            .collect();
    }

    fn toggle_bookmark(&mut self, id: u32, output: &mut impl Write) -> io::Result<()> {
        let Some(target) = self
            .tabs
            .iter()
            .find(|record| record.id == id)
            .and_then(Self::bookmark_target)
        else {
            return Ok(());
        };
        let store = Bookmarks::new(worminal_config::bookmarks_path());
        store.toggle(Bookmark {
            target,
            title: None,
        })?;
        self.bookmarks = store.list()?;
        self.update_tab_entries();
        self.draw_tabs(output)?;
        output.flush()
    }

    fn sync_palette(&mut self) -> bool {
        let overrides = self
            .tabs
            .iter()
            .find(|tab| tab.id == self.active)
            .map_or(&[][..], |tab| tab.colors.as_slice());
        if self.palette_overrides != overrides {
            self.palette_overrides = overrides.to_vec();
            self.palette = screen::palette(overrides);
            self.screen.palette(self.palette.clone());
            return true;
        }
        false
    }

    fn draw_tabs(&mut self, output: &mut impl Write) -> io::Result<()> {
        let old = self.tab_bar.rows.max(1);
        let bar = tabs::draw(
            &self.tab_entries,
            self.active,
            self.size.0,
            self.size.1 - 1,
            |id| self.severity(id),
            &self.palette,
        )?;
        let changed = self.tab_bar.bytes != bar.bytes;
        self.tab_bar = bar;
        if old != self.tab_bar.rows {
            // The shell's origin moved. Its previous damage baseline cannot repaint a new origin.
            output.write_all(b"\x1b[0m\x1b[2J")?;
            self.screen.reset(self.body_size(), output)?;
            if let Some(tab) = self.tabs.iter().find(|tab| tab.id == self.active) {
                self.screen.modes(tab.mode, self.body_size(), output)?;
            }
            self.reload_frame()?;
        }
        if changed {
            output.write_all(&self.tab_bar.bytes)?;
            self.screen.position_cursor(self.body_size(), output)?;
        }
        Ok(())
    }

    fn resize(&mut self, size: Size, output: &mut impl Write) -> io::Result<()> {
        if size == self.size {
            return Ok(());
        }
        let was_hidden = self.body_size().rows == 0;
        self.size = size;
        self.tab_bar.bytes.clear();
        self.draw_tabs(output)?;
        output.flush()?;
        if was_hidden && self.body_size().rows > 0 && self.active != 0 {
            // A one-line terminal displayed no cells. Reclaim to request a full frame even at 1 PTY row.
            self.send(self.active, CommandKind::Release)?;
        }
        self.focus()
    }

    fn message(&mut self, message: &str, output: &mut impl Write) -> io::Result<()> {
        let size = self.body_size();
        self.screen.reset(size, output)?;
        if size.rows != 0 {
            output.write_all(message.as_bytes())?;
        }
        Ok(())
    }

    fn send(&mut self, tab: u32, kind: CommandKind) -> io::Result<()> {
        self.sender.send(&Command::new(tab, kind))
    }

    fn open(&mut self) -> io::Result<()> {
        if !self.opening {
            let mut launch = Launch::restore(Vec::new());
            (launch.columns, launch.rows) = self.pty_size();
            self.send(
                self.active,
                CommandKind::Open {
                    launch,
                    mode: OpenMode::Subscribe,
                },
            )?;
            self.opening = true;
        }
        Ok(())
    }

    fn select(&mut self, tab: u32, output: &mut impl Write) -> io::Result<()> {
        if self.active != 0 && self.active != tab {
            self.send(self.active, CommandKind::Release)?;
        }
        self.active = tab;
        self.sync_palette();
        let size = self.body_size();
        self.screen.reset(size, output)?;
        if let Some(tab) = self.tabs.iter().find(|record| record.id == tab) {
            self.screen.modes(tab.mode, size, output)?;
        }
        self.focus()
    }

    fn reload_frame(&mut self) -> io::Result<()> {
        if self.active != 0 {
            self.send(self.active, CommandKind::Release)?;
            self.focus()?;
        }
        Ok(())
    }

    fn focus(&mut self) -> io::Result<()> {
        if self.active != 0 {
            let (columns, rows) = self.pty_size();
            self.send(self.active, CommandKind::Focus { columns, rows })?;
        }
        Ok(())
    }

    fn event(&mut self, event: Event, output: &mut impl Write) -> io::Result<()> {
        let size = self.body_size();
        match event.kind {
            EventKind::Catalog(catalog) => {
                let first = !std::mem::replace(&mut self.received_catalog, true);
                self.order = catalog.tabs.iter().map(|tab| tab.id).collect();
                self.tabs = catalog
                    .tabs
                    .into_iter()
                    .filter(|tab| tab.kind == TabKind::Terminal)
                    .collect();
                self.update_tab_entries();
                let palette_changed = self.sync_palette();
                self.attention.retain(|id, _| self.order.contains(id));
                if let Some(tab) = self.tabs.iter().find(|tab| tab.id == self.active) {
                    let reverse_changed = self.screen.modes(tab.mode, size, output)?;
                    if palette_changed || reverse_changed {
                        self.reload_frame()?;
                    }
                } else if let Some(tab) = self.tabs.first() {
                    self.select(tab.id, output)?;
                } else if first {
                    self.open()?;
                } else {
                    self.active = 0;
                    self.message("Ctrl+] t opens a shell. Ctrl+] q detaches.", output)?;
                }
                self.draw_tabs(output)?;
            }
            EventKind::Created => {
                self.opening = false;
                self.select(event.tab, output)?;
                self.draw_tabs(output)?;
            }
            EventKind::Attention(severity) => {
                self.attention.insert(event.tab, severity);
                self.draw_tabs(output)?;
            }
            EventKind::Error(message) => {
                return Err(io::Error::other(screen::safe_text(&message)));
            }
            EventKind::Released if event.tab == self.active => {
                self.message(
                    "Tab attached elsewhere. Press a key to take control.",
                    output,
                )?;
            }
            EventKind::Bell if event.tab == self.active => output.write_all(b"\x07")?,
            kind if event.tab == self.active => self.screen.event(kind, size, output)?,
            _ => {}
        }
        output.flush()
    }

    fn tab_action(&mut self, action: TabAction, output: &mut impl Write) -> io::Result<()> {
        match action {
            TabAction::New => self.open(),
            TabAction::Close if self.active != 0 => self.close(self.active),
            TabAction::Bookmark if self.active != 0 => self.toggle_bookmark(self.active, output),
            TabAction::Cycle {
                previous,
                tinted_only,
            } => {
                let current = self
                    .tabs
                    .iter()
                    .position(|tab| tab.id == self.active)
                    .unwrap_or(0);
                let target = tab_controls::cycle(current, self.tabs.len(), previous, |index| {
                    !tinted_only
                        || self.tab_entries[index].tinted(self.severity(self.tabs[index].id))
                });
                if let Some(index) = target {
                    self.select_tab(self.tabs[index].id, output)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn select_tab(&mut self, id: u32, output: &mut impl Write) -> io::Result<()> {
        if id != self.active {
            self.select(id, output)?;
            self.draw_tabs(output)?;
            output.flush()?;
        }
        Ok(())
    }

    fn close(&mut self, id: u32) -> io::Result<()> {
        self.send(id, CommandKind::Close { receipt: false })
    }

    fn pointer(&mut self, pointer: input::Pointer, output: &mut impl Write) -> io::Result<()> {
        let input::Pointer {
            code,
            column,
            row,
            release,
        } = pointer;
        let top = self.body_size().top;
        let button = code & !0x3c;
        let modifiers = (((code >> 2) & 1) * MOD_SHIFT)
            | (((code >> 3) & 1) * MOD_ALT)
            | (((code >> 4) & 1) * MOD_CONTROL);
        let motion = code & 32 != 0;
        let wheel = matches!(button, MOUSE_WHEEL_UP | MOUSE_WHEEL_DOWN);
        if wheel && !release && tab_controls::wheel_cycles(modifiers, row < top) {
            return self.tab_action(
                TabAction::Cycle {
                    previous: button == MOUSE_WHEEL_UP,
                    tinted_only: false,
                },
                output,
            );
        }
        let target = self.tab_bar.hit(column, row, &self.tab_entries);
        let hit = target.map(|(id, _)| id);
        if release && button == 0 {
            if let Some(id) = self.pressed_tab.take() {
                if let Some(target) = hit.filter(|target| *target != id) {
                    let index = |id| self.order.iter().position(|entry| *entry == id);
                    if let (Some(from), Some(to)) = (index(id), index(target)) {
                        let position = tab_controls::move_position(from, to, from < to);
                        self.send(id, CommandKind::Move { position })?;
                    }
                }
                if hit.is_some() && self.tabs.iter().any(|tab| tab.id == id) {
                    self.select_tab(id, output)?;
                }
                return Ok(());
            }
        }
        if row < top {
            if !release && !motion {
                if let (0, Some((id, true))) = (button, target) {
                    return self.toggle_bookmark(id, output);
                }
                match (button, hit) {
                    (0, hit) => self.pressed_tab = hit,
                    (1, Some(id)) => self.close(id)?,
                    _ => {}
                }
            }
            return Ok(());
        }
        if self.pressed_tab.is_some()
            || self.active == 0
            || column >= self.size.0
            || row >= self.size.1
        {
            return Ok(());
        }
        let row = row - top;
        let kind = if wheel {
            CommandKind::Wheel {
                up: button == MOUSE_WHEEL_UP,
                notches: 1,
                lines: worminal_config::get().scroll_multiplier as u32,
                column,
                row,
                modifiers,
            }
        } else {
            let action = if release {
                MouseAction::Release
            } else if motion {
                MouseAction::Motion
            } else {
                MouseAction::Press
            };
            CommandKind::Mouse {
                button,
                column,
                row,
                modifiers,
                action,
            }
        };
        self.send(self.active, kind)
    }

    fn input(&mut self, action: input::Action, output: &mut impl Write) -> io::Result<bool> {
        use input::Action;
        match action {
            Action::Detach => return Ok(false),
            Action::Tab(action) => self.tab_action(action, output)?,
            Action::Pointer(pointer) => self.pointer(pointer, output)?,
            Action::Key(kind) if self.active != 0 => {
                if !self.screen.live() {
                    self.focus()?;
                }
                self.send(self.active, kind)?;
            }
            Action::Scroll(up) if self.active != 0 => {
                self.send(self.active, CommandKind::Scroll { up, amount: 3 })?;
            }
            Action::Bytes(bytes) if self.active != 0 => {
                if !self.screen.live() {
                    self.focus()?;
                }
                self.send(
                    self.active,
                    CommandKind::Input {
                        bytes,
                        control: Control::Acquire,
                    },
                )?;
            }
            _ => {}
        }
        Ok(true)
    }
}

pub(crate) fn run(connect: impl FnOnce() -> io::Result<UnixStream>) -> io::Result<()> {
    let size = tty::size()?;
    // Use the host locale for the widths of Unicode catalog labels.
    unsafe { libc::setlocale(libc::LC_CTYPE, c"".as_ptr()) };
    let socket = connect()?;
    socket.set_write_timeout(Some(Duration::from_secs(3)))?;
    let signals = reactor::Signals::with_extra(&[libc::SIGWINCH])?;
    let mut terminal = tty::Terminal::enter()?;
    let palette = screen::palette(&[]);
    let mut screen = screen::Screen::default();
    screen.palette(palette.clone());
    let mut view = View {
        sender: Session::new(io::empty(), socket.try_clone()?),
        size,
        active: 0,
        tabs: Vec::new(),
        tab_bar: tabs::Bar::default(),
        tab_entries: Vec::new(),
        bookmarks: Bookmarks::new(worminal_config::bookmarks_path())
            .list()
            .unwrap_or_default(),
        attention: HashMap::new(),
        palette,
        palette_overrides: Vec::new(),
        order: Vec::new(),
        pressed_tab: None,
        screen,
        input: input::Input::default(),
        opening: false,
        received_catalog: false,
    };
    view.send(
        0,
        CommandKind::Capabilities(WIRE_FEATURE_ATTENTION | WIRE_FEATURE_HOSTS),
    )?;
    view.send(0, CommandKind::Subscribe)?;
    drive(socket, &signals, &mut view, &mut terminal.output)
}

fn drive(
    mut socket: UnixStream,
    signals: &reactor::Signals,
    view: &mut View,
    output: &mut impl Write,
) -> io::Result<()> {
    let interests = [0, socket.as_raw_fd(), signals.fd()].map(|fd| Interest {
        fd,
        read: true,
        write: false,
    });
    let mut decoder = Decoder::default();
    let mut buffer = [0; 16384];
    let mut escape_deadline: Option<Instant> = None;
    loop {
        let now = Instant::now();
        if escape_deadline.is_some_and(|deadline| deadline <= now) {
            let action = view.input.flush_escape();
            view.input(action, output)?;
            escape_deadline = None;
        }
        let timeout = escape_deadline.map(|deadline| (deadline - now).as_nanos() as i64);
        for ready in reactor::wait(&interests, timeout)? {
            match ready.index {
                0 => {
                    let count = io::stdin().read(&mut buffer)?;
                    if count == 0 {
                        return Ok(());
                    }
                    let actions = view.input.feed(&buffer[..count]);
                    escape_deadline = view
                        .input
                        .pending()
                        .then(|| Instant::now() + Duration::from_millis(40));
                    for action in actions {
                        if !view.input(action, output)? {
                            return Ok(());
                        }
                    }
                }
                1 => {
                    let count = socket.read(&mut buffer)?;
                    if count == 0 {
                        return Err(io::Error::other("Workspace connection closed."));
                    }
                    let mut bytes = &buffer[..count];
                    while !bytes.is_empty() {
                        let (used, packet) = decoder.push(bytes)?;
                        bytes = &bytes[used..];
                        if let Some(event) = packet.map(v2_events::decode).transpose()?.flatten() {
                            view.event(event, output)?;
                        }
                    }
                }
                _ => {
                    if signals.shutdown_requested()? {
                        return Ok(());
                    }
                    view.resize(tty::size()?, output)?;
                }
            }
        }
    }
}
