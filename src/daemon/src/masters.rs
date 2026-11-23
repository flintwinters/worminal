//! Master daemons this daemon shows. Each master has one SSH link owned here, never by a window: the
//! link's catalog becomes local tab records (`Tab::remote`) with local ids, so every view of this daemon
//! sees master tabs exactly as it sees local ones, and a new window already knows them.
//!
//! The master sees this daemon as one view. Each master tab has at most one local controller, so frames
//! pass through unchanged apart from the tab id. `reconcile` keeps the master's view equal to the local
//! one: a tab some local view shows is a `PANE` there (never `FOCUS`, which would hide the master tabs
//! other windows show) and a hidden one is released. A new local controller has no baseline, so it is
//! claimed afresh (`RELEASE` + `PANE`), which makes the master send a full frame.
use crate::{
    queue::Queue,
    service::Service,
    state::{ClientId, Tab, CLIENT_QUEUE},
};
use std::{
    collections::VecDeque,
    ffi::OsStr,
    io::{self, Read},
    os::unix::{ffi::OsStrExt, net::UnixStream},
    path::PathBuf,
};
use worminal_protocol::{
    worp::{
        commands::{Command, CommandKind, Control, MasterOperation},
        events::{split_directory_field, Catalog, Event, EventKind, TabKind},
        launch::Launch,
        v2_commands, v2_events,
    },
    Decoder, Packet, *,
};

/// Features this daemon asks a master for: the frames it forwards must suit every window.
const FEATURES: u32 = WIRE_FEATURE_COPY_ROWS
    | WIRE_FEATURE_ROW_SPAN
    | WIRE_FEATURE_FOCUSED_FRAMES
    | WIRE_FEATURE_NOTIFICATIONS
    | WIRE_FEATURE_ATTENTION
    | WIRE_FEATURE_ROW_LINKS
    | WIRE_FEATURE_PANES
    | WIRE_FEATURE_INPUT
    | WIRE_FEATURE_SELECT_ALL
    // Names the master's own master tabs, which are never imported: chains of masters (or a daemon
    // attached to itself) would otherwise mirror mirrors.
    | WIRE_FEATURE_HOSTS;
/// Frame encodings a view must read to receive a master's frames unchanged.
const FRAME_FEATURES: u32 = WIRE_FEATURE_COPY_ROWS | WIRE_FEATURE_ROW_SPAN | WIRE_FEATURE_ROW_LINKS;
const RETRY_INITIAL_NS: i64 = 250_000_000;
const RETRY_MAX_NS: i64 = 30_000_000_000;

/// A local record of one master tab.
pub(crate) struct RemoteTab {
    pub master: u64,
    pub host: Vec<u8>,
    /// The master's id for the tab.
    pub id: u32,
    pub columns: u32,
    pub rows: u32,
    /// What the master was last told: shown for this local controller at this size, or not shown.
    sent: Option<(ClientId, u32, u32)>,
    /// The view receiving the frame in progress, fixed at its start so a frame is never split.
    target: Option<ClientId>,
}

pub(crate) struct Master {
    pub serial: u64,
    pub host: Vec<u8>,
    link: Option<Link>,
    retry_at: i64,
    retry_delay: i64,
    /// Views awaiting `CREATED` for tabs they opened on the master, in request order, with the launch size.
    opens: VecDeque<(ClientId, u32, u32)>,
    alerted: Option<String>,
}

struct Link {
    socket: UnixStream,
    _child: worminal_workspace::spawn::Child,
    report: io::PipeReader,
    decoder: Decoder,
    output: Queue,
    /// The master's first catalog arrived: SSH authentication and version sync finished.
    live: bool,
}

impl Master {
    fn new(serial: u64, host: Vec<u8>) -> Self {
        Self {
            serial,
            host,
            link: None,
            retry_at: 0,
            retry_delay: RETRY_INITIAL_NS,
            opens: VecDeque::new(),
            alerted: None,
        }
    }
    pub fn live(&self) -> bool {
        self.link.as_ref().is_some_and(|link| link.live)
    }
    fn connect(&mut self, interactive: bool) -> io::Result<()> {
        // The window that started this daemon names its executable; otherwise the installed one beside it.
        let askpass = match std::env::var_os(worminal_workspace::ssh::GUI_ENV) {
            Some(gui) => std::path::PathBuf::from(gui),
            None => std::env::current_exe()?.with_file_name("worminal"),
        };
        let (socket, child, report) = worminal_workspace::ssh::connect_gui(
            OsStr::from_bytes(&self.host),
            // Attaching a master starts its daemon when none runs, as a local window starts this one.
            false,
            askpass.as_os_str(),
            interactive,
        )
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("cannot start the link helper beside {}: {error}", askpass.display()),
            )
        })?;
        socket.set_nonblocking(true)?;
        let mut link = Link {
            socket,
            _child: child,
            report,
            decoder: Decoder::default(),
            output: Queue::new(CLIENT_QUEUE),
            live: false,
        };
        for kind in [CommandKind::Capabilities(FEATURES), CommandKind::Subscribe] {
            let packet = v2_commands::encode(&Command { tab: 0, kind })?;
            link.output.append(&packet.encode()?)?;
        }
        self.link = Some(link);
        self.opens.clear();
        Ok(())
    }
    fn send(&mut self, tab: u32, kind: CommandKind) -> io::Result<()> {
        let link = self.link.as_mut().ok_or(io::ErrorKind::NotConnected)?;
        let packet = v2_commands::encode(&Command { tab, kind })?;
        link.output.append(&packet.encode()?)
    }
    pub fn fd(&self) -> Option<(std::os::fd::RawFd, bool)> {
        use std::os::fd::AsRawFd;
        self.link
            .as_ref()
            .map(|link| (link.socket.as_raw_fd(), !link.output.is_empty()))
    }
}

impl Service {
    fn master_position(&self, serial: u64) -> Option<usize> {
        self.masters
            .iter()
            .position(|master| master.serial == serial)
    }

    /// Attach, detach or close a master for a view, answered with `COMMAND_DONE` or `ERROR`.
    pub fn master_command(
        &mut self,
        client: ClientId,
        operation: MasterOperation,
        host: Vec<u8>,
        launch: Option<Box<Launch>>,
        now: i64,
    ) -> io::Result<()> {
        let result = self.master_operation(client, operation, host, launch, now);
        self.emit(
            client,
            Event {
                tab: 0,
                kind: match result {
                    Ok(()) => EventKind::CommandDone,
                    Err(error) => EventKind::Error(error.to_string().into_bytes()),
                },
            },
        );
        self.publish(now)
    }

    fn master_operation(
        &mut self,
        client: ClientId,
        operation: MasterOperation,
        host: Vec<u8>,
        launch: Option<Box<Launch>>,
        now: i64,
    ) -> io::Result<()> {
        let position = self.masters.iter().position(|master| master.host == host);
        match operation {
            MasterOperation::Attach | MasterOperation::AttachQuietly => {
                let position = position.unwrap_or_else(|| {
                    self.next_master += 1;
                    self.masters.push(Master::new(self.next_master, host));
                    self.masters.len() - 1
                });
                let master = &mut self.masters[position];
                // An explicit attachment retries a link that is not up yet, now and with a prompt.
                if !master.live() {
                    master.link = None;
                    master.retry_delay = RETRY_INITIAL_NS;
                    master.retry_at = now;
                    master.alerted = None;
                    let interactive = operation == MasterOperation::Attach;
                    if let Err(error) = master.connect(interactive) {
                        self.master_lost(self.masters[position].serial, &error.to_string(), now);
                    }
                }
                self.save_masters();
                match launch {
                    // A connecting link queues the open behind its subscription.
                    Some(launch) if self.masters[position].link.is_some() => {
                        self.send_open(position, client, *launch, now);
                        Ok(())
                    }
                    Some(_) => Err(io::Error::new(
                        io::ErrorKind::NotConnected,
                        "The master is not reachable yet; it stays attached and its tabs appear when it is",
                    )),
                    None => Ok(()),
                }
            }
            MasterOperation::Detach => {
                let position = position.ok_or_else(|| self.unknown_master(&host))?;
                let master = self.masters.remove(position);
                while let Some(index) = self.tabs.iter().position(|tab| {
                    tab.remote
                        .as_ref()
                        .is_some_and(|remote| remote.master == master.serial)
                }) {
                    self.close(index);
                }
                self.save_masters();
                Ok(())
            }
            MasterOperation::Close => {
                let position = position.ok_or_else(|| self.unknown_master(&host))?;
                if !self.masters[position].live() {
                    return Err(io::Error::new(
                        io::ErrorKind::NotConnected,
                        "Master is not connected; its tabs cannot be closed now",
                    ));
                }
                let serial = self.masters[position].serial;
                let ids: Vec<u32> = self
                    .tabs
                    .iter()
                    .filter_map(|tab| tab.remote.as_ref())
                    .filter(|remote| remote.master == serial)
                    .map(|remote| remote.id)
                    .collect();
                if ids.is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        "No connected tabs for master",
                    ));
                }
                // The master's catalog removes them, as it does for any other view's close.
                for id in ids {
                    self.masters[position].send(id, CommandKind::Close { receipt: false })?;
                }
                Ok(())
            }
        }
    }

    fn unknown_master(&self, host: &[u8]) -> io::Error {
        let host = String::from_utf8_lossy(host);
        let names: Vec<_> = self
            .masters
            .iter()
            .map(|master| String::from_utf8_lossy(&master.host).into_owned())
            .collect();
        io::Error::new(
            io::ErrorKind::NotFound,
            if names.is_empty() {
                format!("Master {host} is not attached; this daemon has no masters")
            } else {
                format!(
                    "Master {host} is not attached; use the exact endpoint of one of: {}",
                    names.join(", ")
                )
            },
        )
    }

    /// Forward a view's command for a master tab. Control, visibility and order stay local.
    pub fn remote_command(
        &mut self,
        index: usize,
        client: ClientId,
        command: Command,
        now: i64,
    ) -> io::Result<()> {
        let remote = self.tabs[index].remote.as_ref().unwrap();
        let (serial, id) = (remote.master, remote.id);
        let kind = match command.kind {
            CommandKind::Focus { columns, rows }
            | CommandKind::Pane { columns, rows }
            | CommandKind::Resize { columns, rows } => {
                if matches!(command.kind, CommandKind::Resize { .. })
                    && self.tabs[index].controller != Some(client)
                {
                    return Ok(());
                }
                self.take_control(index, client);
                if matches!(command.kind, CommandKind::Focus { .. }) {
                    self.show(index, client);
                } else {
                    self.tabs[index].shown = true;
                }
                let remote = self.tabs[index].remote.as_mut().unwrap();
                remote.columns = columns;
                remote.rows = rows;
                return self.reconcile_masters(now);
            }
            CommandKind::Release => {
                if self.tabs[index].controller == Some(client) {
                    self.release(index);
                }
                return self.reconcile_masters(now);
            }
            CommandKind::Close { .. } => CommandKind::Close { receipt: false },
            // Agent calls and their receipts are answered by the master's own API.
            CommandKind::Call(_)
            | CommandKind::Input {
                control: Control::Preserve,
                ..
            } => {
                return self.error(client, b"Address master tabs through the master's own API.");
            }
            kind @ (CommandKind::Input { .. }
            | CommandKind::Key { .. }
            | CommandKind::Mouse { .. }
            | CommandKind::Paste { .. }
            | CommandKind::Wheel { .. }
            | CommandKind::Scroll { .. }
            | CommandKind::Selection { .. }) => {
                // Input acquires control as it does locally; the claim fetches a full frame.
                self.take_control(index, client);
                self.reconcile_masters(now)?;
                kind
            }
            kind @ (CommandKind::Control(_)
            | CommandKind::FocusState(_)
            | CommandKind::Navigate(_)) => kind,
            _ => return Ok(()),
        };
        if let Some(position) = self.master_position(serial) {
            if self.masters[position].live() {
                if let Err(error) = self.masters[position].send(id, kind) {
                    self.master_lost(serial, &error.to_string(), now);
                }
            }
        }
        Ok(())
    }

    /// Open a terminal on the master that owns tab `near`; its `CREATED` returns to `client`.
    pub fn remote_open(
        &mut self,
        near: usize,
        client: ClientId,
        launch: Launch,
        now: i64,
    ) -> io::Result<()> {
        let serial = self.tabs[near].remote.as_ref().unwrap().master;
        let Some(position) = self
            .master_position(serial)
            .filter(|p| self.masters[*p].live())
        else {
            return self.error(
                client,
                b"Master is not connected; open the tab when it returns.",
            );
        };
        self.send_open(position, client, launch, now);
        Ok(())
    }

    /// Open a terminal on a master for `client`, whose `Created` follows the master's. A lost link answers
    /// every open still pending with an error, so a view never waits for a tab that will not come.
    fn send_open(&mut self, position: usize, client: ClientId, launch: Launch, now: i64) {
        self.masters[position]
            .opens
            .push_back((client, launch.columns, launch.rows));
        let kind = CommandKind::Open {
            launch,
            mode: worminal_protocol::worp::commands::OpenMode::Subscribe,
        };
        if let Err(error) = self.masters[position].send(0, kind) {
            let serial = self.masters[position].serial;
            self.master_lost(serial, &error.to_string(), now);
        }
    }

    /// Make each master show exactly the tabs local views show, at their sizes.
    pub fn reconcile_masters(&mut self, now: i64) -> io::Result<()> {
        let mut lost = Vec::new();
        for index in 0..self.tabs.len() {
            let tab = &self.tabs[index];
            let Some(remote) = &tab.remote else { continue };
            let Some(position) = self
                .master_position(remote.master)
                .filter(|p| self.masters[*p].live())
            else {
                continue;
            };
            let wanted = tab
                .controller
                .filter(|_| tab.shown && remote.columns != 0)
                .map(|client| (client, remote.columns, remote.rows));
            if wanted == remote.sent {
                continue;
            }
            let (id, sent) = (remote.id, remote.sent);
            let master = &mut self.masters[position];
            let result = (|| {
                // A different view, or none, held the master's baseline: claim afresh for a full frame.
                if sent.is_some() && sent.map(|s| s.0) != wanted.map(|w| w.0) {
                    master.send(id, CommandKind::Release)?;
                }
                if let Some((_, columns, rows)) = wanted {
                    master.send(id, CommandKind::Pane { columns, rows })?;
                }
                Ok::<_, io::Error>(())
            })();
            match result {
                Ok(()) => self.tabs[index].remote.as_mut().unwrap().sent = wanted,
                Err(error) => lost.push((master.serial, error.to_string())),
            }
        }
        for (serial, error) in lost {
            self.master_lost(serial, &error, now);
        }
        Ok(())
    }

    /// Start due reconnections; returns the earliest pending retry.
    pub fn retry_masters(&mut self, now: i64) -> Option<i64> {
        let mut next = None::<i64>;
        for position in 0..self.masters.len() {
            let master = &mut self.masters[position];
            if master.link.is_some() {
                continue;
            }
            if master.retry_at <= now {
                // Background retries never prompt: a dialog must follow an explicit attachment.
                if let Err(error) = master.connect(false) {
                    let serial = master.serial;
                    self.master_lost(serial, &error.to_string(), now);
                }
            }
            let master = &self.masters[position];
            if master.link.is_none() {
                next = Some(next.map_or(master.retry_at, |n| n.min(master.retry_at)));
            }
        }
        next
    }

    pub fn master_ready(
        &mut self,
        serial: u64,
        read: bool,
        write: bool,
        now: i64,
    ) -> io::Result<()> {
        let Some(position) = self.master_position(serial) else {
            return Ok(());
        };
        if write {
            let link = self.masters[position].link.as_mut().unwrap();
            if let Err(error) = link
                .output
                .flush(&mut link.socket, WORKSPACE_IO_BYTES as usize)
            {
                self.master_lost(serial, &error.to_string(), now);
                return Ok(());
            }
        }
        if !read {
            return Ok(());
        }
        let mut bytes = vec![0u8; WORKSPACE_IO_BYTES as usize];
        let link = self.masters[position].link.as_mut().unwrap();
        let count = match link.socket.read(&mut bytes) {
            Ok(0) => {
                self.master_lost(serial, "the connection closed", now);
                return Ok(());
            }
            Ok(count) => count,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                return Ok(())
            }
            Err(error) => {
                self.master_lost(serial, &error.to_string(), now);
                return Ok(());
            }
        };
        let mut bytes = &bytes[..count];
        while !bytes.is_empty() {
            let Some(position) = self.master_position(serial) else {
                break;
            };
            let Some(link) = self.masters[position].link.as_mut() else {
                break;
            };
            let (used, packet) = match link.decoder.push(bytes) {
                Ok(decoded) => decoded,
                Err(error) => {
                    self.master_lost(serial, &error.to_string(), now);
                    break;
                }
            };
            bytes = &bytes[used..];
            if let Some(packet) = packet {
                if let Err(error) = self.master_packet(serial, packet, now) {
                    self.master_lost(serial, &error.to_string(), now);
                    break;
                }
            }
        }
        Ok(())
    }

    fn master_packet(&mut self, serial: u64, packet: Packet, now: i64) -> io::Result<()> {
        if packet.kind == WIRE_CATALOG {
            let Some(Event {
                kind: EventKind::Catalog(catalog),
                ..
            }) = v2_events::decode(packet)?
            else {
                return Ok(());
            };
            return self.merge_catalog(serial, catalog, now);
        }
        let position = self.master_position(serial).unwrap();
        let index = (packet.tab != 0)
            .then(|| {
                self.tabs.iter().position(|tab| {
                    tab.remote
                        .as_ref()
                        .is_some_and(|remote| remote.master == serial && remote.id == packet.tab)
                })
            })
            .flatten();
        match packet.kind {
            WIRE_CREATED => {
                // The master's catalog listing the tab follows its reply, so the record starts here.
                let client = self.masters[position].opens.pop_front();
                let index = match index {
                    Some(index) => Some(index),
                    None if packet.tab != 0 => Some(self.remote_record(serial, packet.tab, b"")?.0),
                    None => None,
                };
                if let (Some((client, columns, rows)), Some(index)) = (client, index) {
                    // The master made this link the tab's view at the launch size, as a local open makes
                    // the opener the view: output the tab writes at once (a printer stream) must reach it.
                    self.take_control(index, client);
                    self.show(index, client);
                    let remote = self.tabs[index].remote.as_mut().unwrap();
                    (remote.columns, remote.rows) = (columns, rows);
                    remote.sent = (columns != 0).then_some((client, columns, rows));
                    let id = self.tabs[index].id;
                    self.emit(
                        client,
                        Event {
                            tab: id,
                            kind: EventKind::Created,
                        },
                    );
                }
            }
            WIRE_ERROR if packet.tab == 0 => {
                crate::process::diagnostic(&format!(
                    "Master {}: {}",
                    String::from_utf8_lossy(&self.masters[position].host),
                    String::from_utf8_lossy(&packet.body)
                ));
            }
            WIRE_ATTENTION => {
                if let (
                    Some(index),
                    Some(Event {
                        kind: EventKind::Attention(severity),
                        ..
                    }),
                ) = (index, v2_events::decode(packet)?)
                {
                    self.set_attention(index, severity, None);
                }
            }
            WIRE_RELEASE => {
                // Another view of the master took the tab; local views follow, as for a local tab.
                if let Some(index) = index {
                    self.tabs[index].remote.as_mut().unwrap().sent = None;
                    if let Some(client) = self.tabs[index].controller {
                        self.release(index);
                        let id = self.tabs[index].id;
                        self.queue(client, WIRE_RELEASE, id, []);
                    }
                }
            }
            WIRE_FRAME | WIRE_ROW | WIRE_ROW_SPAN | WIRE_COPY_ROWS | WIRE_ROW_LINKS
            | WIRE_FRAME_FINISH | WIRE_CLIPBOARD | WIRE_BELL | WIRE_NOTIFICATION | WIRE_PRINT
            | WIRE_SELECT_ALL => {
                if let Some(index) = index {
                    self.relay(index, packet);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Pass a master's frame or per-tab packet to the local view it belongs to, under the local tab id.
    fn relay(&mut self, index: usize, packet: Packet) {
        let tab = &mut self.tabs[index];
        let remote = tab.remote.as_mut().unwrap();
        if packet.kind == WIRE_FRAME {
            // Only the view the master is drawing for may take the frame, and only whole.
            remote.target = remote.sent.map(|sent| sent.0).filter(|client| {
                Some(*client) == tab.controller
                    && self.clients.get(client).is_some_and(|c| {
                        c.features & FRAME_FEATURES == FRAME_FEATURES
                            && c.output.len() < WORKSPACE_FRAME_BACKLOG_BYTES as usize
                    })
            });
            if remote.target.is_none() && remote.sent.is_some() {
                // A congested view lost this delta: its next frame must be full.
                remote.sent = None;
            }
        }
        let frame = matches!(
            packet.kind,
            WIRE_ROW | WIRE_ROW_SPAN | WIRE_COPY_ROWS | WIRE_ROW_LINKS | WIRE_FRAME
        );
        let client = if frame || packet.kind == WIRE_FRAME_FINISH {
            remote.target
        } else {
            tab.controller
        };
        if packet.kind == WIRE_FRAME_FINISH {
            remote.target = None;
        }
        let id = tab.id;
        if let Some(client) = client {
            self.queue(client, packet.kind, id, &packet.body);
        }
    }

    /// The local record of master `serial`'s tab `id`, appended when new (the `bool`).
    fn remote_record(&mut self, serial: u64, id: u32, title: &[u8]) -> io::Result<(usize, bool)> {
        if let Some(index) = self.tabs.iter().position(|tab| {
            tab.remote
                .as_ref()
                .is_some_and(|remote| remote.master == serial && remote.id == id)
        }) {
            return Ok((index, false));
        }
        let host = self.masters[self.master_position(serial).unwrap()]
            .host
            .clone();
        let mut tab = Tab::record(self.new_id()?, title);
        tab.remote = Some(RemoteTab {
            master: serial,
            host,
            id,
            columns: 0,
            rows: 0,
            sent: None,
            target: None,
        });
        self.tabs.push(tab);
        Ok((self.tabs.len() - 1, true))
    }

    /// Import a master's catalog: update known records, add new ones, close vanished ones.
    fn merge_catalog(&mut self, serial: u64, catalog: Catalog, now: i64) -> io::Result<()> {
        let position = self.master_position(serial).unwrap();
        let master = &mut self.masters[position];
        let first = !master.link.as_ref().unwrap().live;
        master.link.as_mut().unwrap().live = true;
        master.retry_delay = RETRY_INITIAL_NS;
        master.alerted = None;
        let mut changed = false;
        let catalog = Catalog {
            tabs: catalog
                .tabs
                .into_iter()
                .filter(|record| record.host.is_empty())
                .collect(),
            ..catalog
        };
        for record in &catalog.tabs {
            let (index, created) = self.remote_record(serial, record.id, &record.title)?;
            changed |= created;
            let tab = &mut self.tabs[index];
            let (directory, user) = split_directory_field(&record.directory);
            tab.title.clone_from(&record.title);
            tab.directory = directory.to_vec();
            tab.ssh_user = user.map(<[u8]>::to_vec);
            tab.url = (record.kind == TabKind::Browser).then(|| record.url.clone());
            tab.mode = record.mode;
            tab.cursor = record.cursor;
            tab.colors = Some(
                record
                    .colors
                    .iter()
                    .map(|color| (!color.is_empty()).then(|| color.clone()))
                    .collect(),
            );
            tab.catalog_dirty = true;
        }
        let mut index = 0;
        while index < self.tabs.len() {
            let gone = self.tabs[index].remote.as_ref().is_some_and(|remote| {
                remote.master == serial && !catalog.tabs.iter().any(|tab| tab.id == remote.id)
            });
            if gone {
                self.close(index);
                changed = true;
            } else {
                index += 1;
            }
        }
        if first {
            // A new link holds no baselines: every shown master tab is claimed afresh.
            for tab in &mut self.tabs {
                if let Some(remote) = tab.remote.as_mut().filter(|r| r.master == serial) {
                    remote.sent = None;
                    remote.target = None;
                }
            }
        }
        if changed {
            self.save_masters();
        }
        self.publish(now)?;
        self.reconcile_masters(now)
    }

    /// The link failed: keep the master's tabs, report why once, and retry with backoff.
    pub fn master_lost(&mut self, serial: u64, error: &str, now: i64) {
        let Some(position) = self.master_position(serial) else {
            return;
        };
        let master = &mut self.masters[position];
        let report = master.link.take().and_then(|mut link| {
            let mut text = Vec::new();
            let _ = (&mut link.report).take(4096).read_to_end(&mut text);
            let text = String::from_utf8_lossy(&text).trim().to_owned();
            (!text.is_empty()).then_some(text)
        });
        let message = report.unwrap_or_else(|| error.to_owned());
        let host = String::from_utf8_lossy(&master.host).into_owned();
        // An unreachable host is retried quietly; anything else (a failed sync) is told once.
        let alert = !message.starts_with(worminal_workspace::ssh::UNREACHABLE_REPORT)
            && master.alerted.as_deref() != Some(&message);
        if alert {
            master.alerted = Some(message.clone());
        }
        let opens = std::mem::take(&mut master.opens);
        master.retry_at = now + master.retry_delay;
        master.retry_delay = (master.retry_delay * 2).min(RETRY_MAX_NS);
        crate::process::diagnostic(&format!("Master {host} disconnected: {message}"));
        for tab in &mut self.tabs {
            if let Some(remote) = tab.remote.as_mut().filter(|r| r.master == serial) {
                remote.sent = None;
                remote.target = None;
            }
        }
        for (client, ..) in opens {
            self.emit(
                client,
                Event {
                    tab: 0,
                    kind: EventKind::Error(
                        format!("Master {host} disconnected before opening the tab: {message}")
                            .into_bytes(),
                    ),
                },
            );
        }
        if alert {
            let text = format!("Master {host}: {message}").into_bytes();
            let clients: Vec<_> = self
                .clients
                .values()
                .filter(|client| client.ready)
                .map(|client| client.id)
                .collect();
            for client in clients {
                self.emit(
                    client,
                    Event {
                        tab: 0,
                        kind: EventKind::Notification(text.clone()),
                    },
                );
            }
        }
    }

    /// Attachments outlive this daemon: an upgrade or restart reconnects them and keeps their tabs'
    /// local ids and positions, so views never see master tabs disappear and return.
    pub fn save_masters(&self) {
        let Some(path) = &self.masters_path else {
            return;
        };
        let text = |bytes: &[u8]| serde_json::Value::from(String::from_utf8_lossy(bytes));
        let tabs: Vec<_> = self
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(position, tab)| {
                let remote = tab.remote.as_ref()?;
                let master = self.masters.iter().find(|m| m.serial == remote.master)?;
                Some(serde_json::json!({
                    "id": tab.id,
                    "after": position.checked_sub(1).map_or(0, |p| self.tabs[p].id),
                    "host": text(&master.host),
                    "remote": remote.id,
                    "title": text(&tab.title),
                    "directory": text(&tab.directory),
                    "url": tab.url.as_deref().map(text),
                }))
            })
            .collect();
        let hosts: Vec<_> = self.masters.iter().map(|m| text(&m.host)).collect();
        let body = serde_json::json!({ "masters": hosts, "tabs": tabs }).to_string();
        let temporary = path.with_extension("tmp");
        let result = std::fs::create_dir_all(path.parent().unwrap())
            .and_then(|()| std::fs::write(&temporary, body))
            .and_then(|()| std::fs::rename(&temporary, path));
        if let Err(error) = result {
            crate::process::diagnostic(&format!("Master list save: {error}"));
        }
    }

    /// Restore saved attachments and their tab records; links connect without prompting. An upgrade
    /// keeps the records' ids and positions; a fresh daemon's ids are its own, so records append.
    pub fn restore_masters(&mut self, upgrade: bool) {
        let saved = self
            .masters_path
            .as_ref()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
        let Some(saved) = saved else { return };
        let string =
            |value: &serde_json::Value| value.as_str().unwrap_or_default().as_bytes().to_vec();
        for host in saved["masters"].as_array().into_iter().flatten() {
            let host = string(host);
            if worminal_protocol::worp::commands::valid_master(&host)
                && !self.masters.iter().any(|m| m.host == host)
            {
                self.next_master += 1;
                self.masters.push(Master::new(self.next_master, host));
            }
        }
        for record in saved["tabs"].as_array().into_iter().flatten() {
            let (Some(id), Some(remote)) = (record["id"].as_u64(), record["remote"].as_u64())
            else {
                continue;
            };
            let (Ok(id), Ok(remote)) = (u32::try_from(id), u32::try_from(remote)) else {
                continue;
            };
            let host = string(&record["host"]);
            let Some(serial) = self
                .masters
                .iter()
                .find(|m| m.host == host)
                .map(|m| m.serial)
            else {
                continue;
            };
            let id = if upgrade {
                if id == 0 || self.index(id).is_some() {
                    continue;
                }
                id
            } else {
                match self.new_id() {
                    Ok(id) => id,
                    Err(_) => break,
                }
            };
            let mut tab = Tab::record(id, &string(&record["title"]));
            tab.directory = string(&record["directory"]);
            tab.url = record["url"].is_string().then(|| string(&record["url"]));
            tab.remote = Some(RemoteTab {
                master: serial,
                host,
                id: remote,
                columns: 0,
                rows: 0,
                sent: None,
                target: None,
            });
            let after = record["after"].as_u64().and_then(|a| u32::try_from(a).ok());
            let position = match after.filter(|_| upgrade) {
                Some(0) => 0,
                Some(after) => self.index(after).map_or(self.tabs.len(), |p| p + 1),
                None => self.tabs.len(),
            };
            self.tabs.insert(position, tab);
            self.next_id = self.next_id.max(id.saturating_add(1));
        }
    }
}

/// Where a daemon scope keeps its master attachments, beside its recovery recipes.
pub(crate) fn path(recovery: &std::path::Path) -> PathBuf {
    let name = recovery.file_name().unwrap_or_default().to_string_lossy();
    recovery.with_file_name(name.replacen("tabs-", "masters-", 1) + ".json")
}

/// Run `operation` on the local tabs only: handoffs carry live processes, never master records,
/// which the receiver restores from the saved master list. Records return to their positions.
pub(crate) fn without_remote<T>(
    tabs: &mut Vec<Tab>,
    operation: impl FnOnce(&mut Vec<Tab>) -> T,
) -> T {
    let mut held = Vec::new();
    let mut index = 0;
    let mut position = 0;
    while index < tabs.len() {
        if tabs[index].remote.is_some() {
            held.push((position, tabs.remove(index)));
        } else {
            index += 1;
        }
        position += 1;
    }
    let result = operation(tabs);
    for (position, tab) in held {
        let position = position.min(tabs.len());
        tabs.insert(position, tab);
    }
    result
}
