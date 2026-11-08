use crate::{
    process::{ChildReaper, Pty},
    queue::Queue,
};
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::{ffi::OsStringExt, fs::OpenOptionsExt},
    path::PathBuf,
};
use worminal_engine::{self as terminal, Engine, Event, Frame, TransportState};
use worminal_protocol::{Buffer, Decoder, WORKSPACE_INITIAL_CURSOR};
use worminal_workspace::launch::Launch;

pub(crate) type ClientId = u64;
pub(crate) const CLIENT_QUEUE: usize = worminal_protocol::WORKSPACE_CLIENT_QUEUE_BYTES as usize;
pub(crate) const INPUT_QUEUE: usize = worminal_protocol::WORKSPACE_INPUT_QUEUE_BYTES as usize;

pub(crate) fn core<T>(value: Result<T, terminal::Error>) -> io::Result<T> {
    value.map_err(|error| io::Error::other(format!("terminal engine status {}", error.0)))
}

pub(crate) struct Client {
    pub id: ClientId,
    pub socket: std::os::unix::net::UnixStream,
    pub pid: libc::pid_t,
    pub ready: bool,
    pub features: u32,
    pub decoder: Decoder,
    pub output: Queue,
    pub needs_resync: bool,
    pub selection: Option<(u32, u32, Vec<u8>, usize)>,
}

impl Client {
    pub fn queue(&mut self, kind: u32, tab: u32, body: &[u8]) -> io::Result<()> {
        // Preflight the complete transport budget before copying potentially large imported metadata.
        let mut encoded = Buffer::with_limit(self.output.remaining());
        encoded.append_packet_bytes(kind, tab, body)?;
        self.output.append(encoded.as_bytes())
    }
}

pub(crate) enum Printer {
    Client,
    File(File),
    Disabled,
}

impl Printer {
    fn open(launch: &Launch) -> Self {
        if launch.output.is_empty() || launch.output == b"-" {
            return Self::Client;
        }
        let directory = if launch.directory.is_empty() {
            std::env::var_os("HOME").unwrap_or_else(|| "/".into())
        } else {
            OsString::from_vec(launch.directory.clone())
        };
        let path = PathBuf::from(directory).join(OsString::from_vec(launch.output.clone()));
        match OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .mode(0o666)
            .open(path)
        {
            Ok(file) => Self::File(file),
            Err(error) => {
                eprintln!("Error opening terminal printer: {error}");
                Self::Disabled
            }
        }
    }

    pub fn kind(&self) -> i32 {
        match self {
            Self::Client => 1,
            Self::File(_) => 0,
            Self::Disabled => -1,
        }
    }
}

pub(crate) struct Terminal {
    pub engine: Engine,
    pub pty: Pty,
    pub pending: Vec<u8>,
    pub remote_scroll: i32,
    pub input: Queue,
    pub printer: Printer,
}

#[derive(Default)]
pub(crate) struct Screen {
    pub frame: Frame,
    pub bytes: Vec<u8>,
}

pub(crate) struct Tab {
    pub id: u32,
    pub terminal: Option<Terminal>,
    pub title: Vec<u8>,
    pub initial_title: Vec<u8>,
    pub directory: Vec<u8>,
    next_output_directory: i64,
    pub clipboard: Vec<u8>,
    pub url: Option<Vec<u8>>,
    pub colors: Option<Vec<Option<Vec<u8>>>>,
    pub mode: u32,
    pub cursor: u32,
    pub controller: Option<ClientId>,
    pub print_client: Option<ClientId>,
    pub screen: Screen,
    pub catalog: Vec<u8>,
    pub catalog_pending: bool,
    pub catalog_dirty: bool,
    pub pending_frame: bool,
    pub input_frame: bool,
    pub frame_reply: bool,
}

pub(crate) enum Notice {
    Packet(ClientId, u32, Vec<u8>),
    Diagnostic(Vec<u8>),
}

impl Tab {
    pub fn shell(
        id: u32,
        launch: &Launch,
        restored: bool,
        client: Option<ClientId>,
        reaper: ChildReaper,
    ) -> io::Result<Self> {
        let mut tab = Self::record(
            id,
            if launch.title.is_empty() {
                b"Worminal"
            } else {
                &launch.title
            },
        );
        tab.directory = if launch.directory.is_empty() {
            std::env::var_os("HOME")
                .unwrap_or_else(|| "/".into())
                .into_vec()
        } else {
            launch.directory.clone()
        };
        tab.cursor = WORKSPACE_INITIAL_CURSOR;
        tab.controller = client;
        if launch.output == b"-" {
            tab.print_client = client;
        }
        let mut engine = core(Engine::new(launch.columns as i32, launch.rows as i32))?;
        core(engine.allow_alternate(launch.allow_alternate))?;
        if !launch.output.is_empty() {
            core(engine.control(terminal::WORMINAL_TOGGLE_PRINTER))?;
        }
        let printer = Printer::open(launch);
        let pty = Pty::spawn(launch, restored, reaper)?;
        tab.terminal = Some(Terminal {
            engine,
            pty,
            pending: Vec::new(),
            remote_scroll: 0,
            input: Queue::new(INPUT_QUEUE),
            printer,
        });
        Ok(tab)
    }

    pub(crate) fn record(id: u32, title: &[u8]) -> Self {
        Self {
            id,
            terminal: None,
            title: title.to_vec(),
            initial_title: title.to_vec(),
            directory: Vec::new(),
            next_output_directory: 0,
            clipboard: Vec::new(),
            url: None,
            colors: None,
            mode: 0,
            cursor: 0,
            controller: None,
            print_client: None,
            screen: Screen::default(),
            catalog: Vec::new(),
            catalog_pending: false,
            catalog_dirty: true,
            pending_frame: false,
            input_frame: false,
            frame_reply: false,
        }
    }

    pub fn browser(id: u32, url: Vec<u8>) -> Self {
        let mut tab = Self::record(id, b"Browser");
        tab.url = Some(url);
        tab
    }

    pub fn release(&mut self) {
        self.controller = None;
        self.frame_reply = false;
        self.screen = Screen::default();
    }

    pub fn frame(&mut self, now: i64) -> io::Result<Option<Frame>> {
        self.terminal
            .as_mut()
            .map(|terminal| core(terminal.engine.frame(now)))
            .transpose()
    }

    pub fn refresh_directory_after_output(&mut self, now: i64) {
        if now >= self.next_output_directory {
            self.refresh_directory(now);
        }
    }

    pub fn refresh_directory(&mut self, now: i64) {
        // Output can arrive in thousands of tiny reads. Coalesce those checks,
        // but let the periodic service poll bypass this gate so it retains its
        // existing observation interval even after an early output check.
        self.next_output_directory =
            now.saturating_add(worminal_protocol::WORKSPACE_DIRECTORY_INTERVAL_NS as i64);
        let Some(terminal) = &mut self.terminal else {
            return;
        };
        let pid = terminal.pty.live_pid();
        let directory = if pid > 0 {
            fs::read_link(format!("/proc/{pid}/cwd")).ok()
        } else {
            None
        };
        let bytes = directory.map_or_else(Vec::new, |path| path.into_os_string().into_vec());
        if self.directory != bytes {
            self.directory = bytes;
            self.catalog_dirty = true;
        }
    }

    pub fn metadata(&mut self, now: i64) -> io::Result<&[u8]> {
        if !self.catalog_dirty && !self.catalog_pending && !self.catalog.is_empty() {
            return Ok(&self.catalog);
        }
        if self
            .frame(now)?
            .is_some_and(|frame| frame.redraw_deadline != 0)
            && !self.catalog.is_empty()
        {
            self.catalog_pending = true;
            return Ok(&self.catalog);
        }
        // Native handoff metadata can expand when default palette strings are restored. Keep
        // packet/queue limits in the transport owner: oversized catalogs must not kill PTY owners.
        let mut buffer = Buffer::with_limit(usize::MAX);
        buffer.put_u32(self.id)?;
        buffer.put_u32(u32::from(self.url.is_some()))?;
        buffer.put_string(self.url.as_deref().unwrap_or_default())?;
        buffer.put_u32(self.mode)?;
        buffer.put_u32(self.cursor)?;
        buffer.put_string(&self.title)?;
        buffer.put_string(&self.directory)?;
        buffer.put_u32(worminal_config::COLOR_NAMES.len() as u32)?;
        for (index, default) in worminal_config::COLOR_NAMES.iter().enumerate() {
            let color = self
                .colors
                .as_ref()
                .and_then(|colors| colors[index].as_deref());
            buffer.put_string(color.unwrap_or_else(|| default.unwrap_or("").as_bytes()))?;
        }
        self.catalog = buffer.into_bytes();
        self.catalog_pending = false;
        self.catalog_dirty = false;
        Ok(&self.catalog)
    }

    pub fn feed(&mut self, bytes: &[u8], now: i64) -> io::Result<Vec<Notice>> {
        if self.terminal.is_none() {
            return Ok(Vec::new());
        }
        let mut notices = Vec::new();
        if self.terminal.as_ref().unwrap().pending.len() >= 4 {
            // Native compatibility snapshots permit a larger unparsed buffer than a live UTF-8 suffix.
            let mut imported = std::mem::take(&mut self.terminal.as_mut().unwrap().pending);
            imported.extend_from_slice(bytes);
            let used = self.consume(&imported, now, &mut notices)?;
            self.terminal
                .as_mut()
                .unwrap()
                .pending
                .extend_from_slice(&imported[used..]);
            self.pending_frame = true;
            return Ok(notices);
        }
        let mut bytes = bytes;
        while !self.terminal.as_ref().unwrap().pending.is_empty() && !bytes.is_empty() {
            let terminal = self.terminal.as_mut().unwrap();
            let prefix = terminal.pending.len();
            let mut leading = [0u8; 4];
            leading[..prefix].copy_from_slice(&terminal.pending);
            terminal.pending.clear();
            let taken = (4 - prefix).min(bytes.len());
            leading[prefix..prefix + taken].copy_from_slice(&bytes[..taken]);
            let used = self.consume(&leading[..prefix + taken], now, &mut notices)?;
            self.terminal
                .as_mut()
                .unwrap()
                .pending
                .extend_from_slice(&leading[used..prefix + taken]);
            bytes = &bytes[taken..];
        }
        if self.terminal.as_ref().unwrap().pending.is_empty() {
            let used = self.consume(bytes, now, &mut notices)?;
            self.terminal
                .as_mut()
                .unwrap()
                .pending
                .extend_from_slice(&bytes[used..]);
        }
        if self.terminal.as_ref().unwrap().pending.len() >= 4 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "engine retained more than one UTF-8 suffix",
            ));
        }
        self.pending_frame = true;
        Ok(notices)
    }

    fn consume(&mut self, bytes: &[u8], now: i64, notices: &mut Vec<Notice>) -> io::Result<usize> {
        let mut offset = 0;
        while offset < bytes.len() {
            let (used, events) = core(
                self.terminal
                    .as_mut()
                    .unwrap()
                    .engine
                    .feed(&bytes[offset..], now),
            )?;
            notices.extend(self.effects(events, now)?);
            if used == 0 {
                break;
            }
            offset += used;
        }
        Ok(offset)
    }

    pub fn input(&mut self, bytes: &[u8], echo: bool, now: i64) -> io::Result<Vec<Notice>> {
        let Some(terminal) = &mut self.terminal else {
            return Ok(Vec::new());
        };
        let effects = core(terminal.engine.input(bytes, echo, now))?;
        self.effects(effects, now)
    }

    pub fn effects(&mut self, events: Vec<Event>, now: i64) -> io::Result<Vec<Notice>> {
        let mut notices = Vec::new();
        for event in events {
            match event.kind {
                terminal::WORMINAL_REPLY => {
                    notices.extend(self.input(&event.data, event.flags != 0, now)?)
                }
                terminal::WORMINAL_WRITE => {
                    let terminal = self.terminal.as_mut().unwrap();
                    if terminal.input.append(&event.data).is_err() {
                        if let Some(client) = self.controller {
                            let mut error = Buffer::default();
                            error.put_string(b"PTY input queue exceeded 1 MiB; tab stopped")?;
                            notices.push(Notice::Packet(
                                client,
                                worminal_protocol::WIRE_ERROR,
                                error.into_bytes(),
                            ));
                        }
                        terminal.pty.stop();
                    }
                }
                terminal::WORMINAL_TITLE => {
                    let title = if event.data.is_empty() {
                        self.initial_title.clone()
                    } else {
                        event.data
                    };
                    if self.title != title {
                        self.title = title;
                        self.catalog_dirty = true;
                    }
                }
                terminal::WORMINAL_MODE => {
                    let next = if event.argument != 0 {
                        self.mode | event.flags
                    } else {
                        self.mode & !event.flags
                    };
                    self.catalog_dirty |= next != self.mode;
                    self.mode = next;
                }
                terminal::WORMINAL_CURSOR => {
                    self.catalog_dirty |= self.cursor != event.argument as u32;
                    self.cursor = event.argument as u32;
                }
                terminal::WORMINAL_PALETTE => {
                    if let Ok(index) = usize::try_from(event.argument) {
                        if index < worminal_config::COLOR_NAMES.len() {
                            let next = (event.flags == 0).then_some(event.data);
                            if self.colors.is_none() && next.is_some() {
                                self.colors = Some(vec![None; worminal_config::COLOR_NAMES.len()]);
                            }
                            if let Some(colors) = &mut self.colors {
                                self.catalog_dirty |= colors[index] != next;
                                colors[index] = next;
                                if colors.iter().all(Option::is_none) {
                                    self.colors = None;
                                }
                            }
                        }
                    }
                }
                terminal::WORMINAL_PALETTE_QUERY => {
                    let number = event.argument;
                    let index = if event.flags != 0 {
                        number
                    } else {
                        match number {
                            10 => terminal::WORMINAL_DEFAULT_FOREGROUND as i32,
                            11 => terminal::WORMINAL_DEFAULT_BACKGROUND as i32,
                            12 => terminal::WORMINAL_DEFAULT_CURSOR as i32,
                            _ => -1,
                        }
                    };
                    if let Some(color) = self.color_rgb(index) {
                        let prefix = if event.flags != 0 { "4;" } else { "" };
                        let [red, green, blue] = color;
                        let reply = format!(
                            concat!("\x1b]{prefix}{number};rgb:",
                            "{red:02x}{red:02x}/{green:02x}{green:02x}/{blue:02x}{blue:02x}\x07"),
                            prefix = prefix,
                            number = number,
                            red = red,
                            green = green,
                            blue = blue
                        );
                        notices.extend(self.input(reply.as_bytes(), true, now)?);
                    }
                }
                terminal::WORMINAL_CLIPBOARD => {
                    self.clipboard = event.data;
                    if let Some(client) = self.controller {
                        let mut body = Buffer::default();
                        body.put_string(&self.clipboard)?;
                        notices.push(Notice::Packet(
                            client,
                            worminal_protocol::WIRE_CLIPBOARD,
                            body.into_bytes(),
                        ));
                    }
                }
                terminal::WORMINAL_BELL => {
                    if let Some(client) = self.controller {
                        notices.push(Notice::Packet(
                            client,
                            worminal_protocol::WIRE_BELL,
                            Vec::new(),
                        ));
                    }
                }
                terminal::WORMINAL_PRINT => {
                    let terminal = self.terminal.as_mut().unwrap();
                    match &mut terminal.printer {
                        Printer::Client => {
                            if let Some(client) = self.print_client {
                                notices.push(Notice::Packet(
                                    client,
                                    worminal_protocol::WIRE_PRINT,
                                    event.data,
                                ));
                            }
                        }
                        Printer::File(file) => {
                            if let Err(error) = file.write_all(&event.data) {
                                notices.push(Notice::Diagnostic(
                                    format!("Error writing terminal printer: {error}").into_bytes(),
                                ));
                                terminal.printer = Printer::Disabled;
                            }
                        }
                        Printer::Disabled => {}
                    }
                }
                terminal::WORMINAL_DIAGNOSTIC => notices.push(Notice::Diagnostic(event.data)),
                _ => {}
            }
        }
        Ok(notices)
    }

    fn color_rgb(&self, index: i32) -> Option<[u8; 3]> {
        let index = usize::try_from(index).ok()?;
        let default = worminal_config::COLOR_NAMES.get(index)?;
        let override_value = self
            .colors
            .as_ref()
            .and_then(|colors| colors[index].as_deref());
        let name = override_value.or_else(|| default.map(str::as_bytes))?;
        if name.first() != Some(&b'#') || name.len() < 7 {
            return None;
        }
        let mut color = [0; 3];
        for (index, output) in color.iter_mut().enumerate() {
            *output = u8::from_str_radix(
                std::str::from_utf8(&name[1 + index * 2..3 + index * 2]).ok()?,
                16,
            )
            .ok()?;
        }
        Some(color)
    }

    pub fn transport(&self) -> TransportState {
        let terminal = self.terminal.as_ref().unwrap();
        let mut state = TransportState {
            pending_bytes: terminal.pending.len() as i32,
            ..Default::default()
        };
        state.pending[..terminal.pending.len()].copy_from_slice(&terminal.pending);
        state.output_kind = terminal.printer.kind();
        state.remote_scroll = terminal.remote_scroll;
        state
    }
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
