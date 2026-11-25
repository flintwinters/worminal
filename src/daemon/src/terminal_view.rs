//! Interactive VT presentation of a Worp session, including over an ordinary SSH PTY.
mod input;
mod screen;
mod tabs;
mod tty;

use crate::reactor::{self, Interest};
use std::{
    io::{self, Read, Write},
    os::{fd::AsRawFd, unix::net::UnixStream},
    time::{Duration, Instant},
};
use worminal_protocol::{
    input::{MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOUSE_WHEEL_DOWN, MOUSE_WHEEL_UP},
    worp::{
        commands::{Command, CommandKind, Control, MouseAction, OpenMode},
        events::{Event, EventKind, TabKind, TabRecord},
        launch::Launch,
        session::Session,
        v2_events,
    },
    Decoder,
};
use worminal_workspace::tab_controls::{self, Action as TabAction};

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

struct View {
    sender: Sender,
    size: Size,
    active: u32,
    tabs: Vec<TabRecord>,
    tab_bar: tabs::Bar,
    order: Vec<u32>,
    pressed_tab: Option<u32>,
    screen: screen::Screen,
    input: input::Input,
    opening: bool,
    received_catalog: bool,
}

impl View {
    fn body_size(&self) -> Size {
        (self.size.0, self.size.1 - tabs::ROWS)
    }

    fn pty_size(&self) -> Size {
        let (columns, rows) = self.body_size();
        (columns, rows.max(1))
    }

    fn draw_tabs(&mut self, output: &mut impl Write) -> io::Result<()> {
        let bar = tabs::draw(&self.tabs, self.active, self.size.0)?;
        if self.tab_bar.bytes != bar.bytes {
            output.write_all(&bar.bytes)?;
            self.screen.position_cursor(self.body_size(), output)?;
        }
        self.tab_bar = bar;
        Ok(())
    }

    fn resize(&mut self, size: Size, output: &mut impl Write) -> io::Result<()> {
        if size == self.size {
            return Ok(());
        }
        let was_hidden = self.body_size().1 == 0;
        self.size = size;
        self.tab_bar.bytes.clear();
        self.draw_tabs(output)?;
        output.flush()?;
        if was_hidden && self.body_size().1 > 0 && self.active != 0 {
            // A one-line terminal displayed no cells. Reclaim to request a full frame even at 1 PTY row.
            self.send(self.active, CommandKind::Release)?;
        }
        self.focus()
    }

    fn message(&mut self, message: &str, output: &mut impl Write) -> io::Result<()> {
        let size = self.body_size();
        self.screen.reset(size, output)?;
        if size.1 != 0 {
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
        let size = self.body_size();
        self.screen.reset(size, output)?;
        if let Some(tab) = self.tabs.iter().find(|record| record.id == tab) {
            self.screen.modes(tab.mode, size, output)?;
        }
        self.focus()
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
                if let Some(tab) = self.tabs.iter().find(|tab| tab.id == self.active) {
                    self.screen.modes(tab.mode, size, output)?;
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
            TabAction::Cycle {
                previous,
                tinted_only,
            } => {
                let current = self
                    .tabs
                    .iter()
                    .position(|tab| tab.id == self.active)
                    .unwrap_or(0);
                use worminal_protocol::cell::{MODE_BACKGROUND_JOB, MODE_BUSY};
                let target = tab_controls::cycle(current, self.tabs.len(), previous, |index| {
                    !tinted_only || self.tabs[index].mode & (MODE_BUSY | MODE_BACKGROUND_JOB) != 0
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
        let button = code & !0x3c;
        let modifiers = (((code >> 2) & 1) * MOD_SHIFT)
            | (((code >> 3) & 1) * MOD_ALT)
            | (((code >> 4) & 1) * MOD_CONTROL);
        let motion = code & 32 != 0;
        let wheel = matches!(button, MOUSE_WHEEL_UP | MOUSE_WHEEL_DOWN);
        if wheel && !release && tab_controls::wheel_cycles(modifiers, row < tabs::ROWS) {
            return self.tab_action(
                TabAction::Cycle {
                    previous: button == MOUSE_WHEEL_UP,
                    tinted_only: false,
                },
                output,
            );
        }
        let hit = (row < tabs::ROWS)
            .then(|| self.tab_bar.hit(column))
            .flatten();
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
        if row < tabs::ROWS {
            if !release && !motion {
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
        let row = row - tabs::ROWS;
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
    let mut view = View {
        sender: Session::new(io::empty(), socket.try_clone()?),
        size,
        active: 0,
        tabs: Vec::new(),
        tab_bar: tabs::Bar::default(),
        order: Vec::new(),
        pressed_tab: None,
        screen: screen::Screen::default(),
        input: input::Input::default(),
        opening: false,
        received_catalog: false,
    };
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
