//! Interactive VT presentation of a Worp session, including over an ordinary SSH PTY.
mod input;
mod screen;
mod tabs;
mod tty;

use crate::reactor::{self, Interest};
use std::{
    io::{self, Read, Write},
    os::{fd::AsRawFd, unix::net::UnixStream},
    time::Duration,
};
use worminal_protocol::{
    worp::{
        commands::{Command, CommandKind, Control, OpenMode},
        events::{Event, EventKind, TabKind, TabRecord},
        launch::Launch,
        session::Session,
        v2_events,
    },
    Decoder,
};

pub(crate) const HELP: &str = "worminald terminal
Open the workspace in a VT-compatible terminal, or run ssh -t USER@HOST worminald terminal.
The Linux host renders the tab bar and shell pane over the terminal. Browser tabs are skipped.
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
    tab_bar: Vec<u8>,
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
        if self.tab_bar != bar {
            output.write_all(&bar)?;
            self.tab_bar = bar;
            self.screen.position_cursor(self.body_size(), output)?;
        }
        Ok(())
    }

    fn resize(&mut self, size: Size, output: &mut impl Write) -> io::Result<()> {
        if size == self.size {
            return Ok(());
        }
        let was_hidden = self.body_size().1 == 0;
        self.size = size;
        self.tab_bar.clear();
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

    fn input(&mut self, action: input::Action, output: &mut impl Write) -> io::Result<bool> {
        use input::Action;
        match action {
            Action::Detach => return Ok(false),
            Action::New => self.open()?,
            Action::Next | Action::Previous if !self.tabs.is_empty() => {
                let index = self
                    .tabs
                    .iter()
                    .position(|tab| tab.id == self.active)
                    .unwrap_or(0);
                let count = self.tabs.len();
                let step = if matches!(action, Action::Next) {
                    1
                } else {
                    count - 1
                };
                let tab = self.tabs[(index + step) % count].id;
                if tab != self.active {
                    self.select(tab, output)?;
                    self.draw_tabs(output)?;
                    output.flush()?;
                }
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
        tab_bar: Vec::new(),
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
    loop {
        for ready in reactor::wait(&interests, None)? {
            match ready.index {
                0 => {
                    let count = io::stdin().read(&mut buffer)?;
                    if count == 0 {
                        return Ok(());
                    }
                    for action in view.input.feed(&buffer[..count]) {
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
