//! Readiness owns stable client/tab identities; recycled descriptor numbers never identify work.
use crate::{
    frames::Outcome,
    process::{self, ChildReaper},
    queue::Queue,
    reactor::{self, Interest, Signals},
    state::{core, Client, ClientId, Notice, Tab, CLIENT_QUEUE},
};
use std::{
    collections::BTreeMap,
    io,
    os::{
        fd::AsRawFd,
        unix::{ffi::OsStringExt, net::UnixListener},
    },
    path::PathBuf,
    time::Instant,
};
use worminal_protocol::*;
use worminal_workspace::{
    launch::Launch,
    recovery::{self, Recipe, Recovery},
    socket,
};

pub(crate) struct Service {
    pub listener: UnixListener,
    pub clients: BTreeMap<ClientId, Client>,
    pub tabs: Vec<Tab>,
    pub next_id: u32,
    pub next_client: ClientId,
    pub reaper: ChildReaper,
    pub running: bool,
    pub restored_launch: bool,
    pub pending_replace: BTreeMap<ClientId, (u32, Vec<u8>)>,
    pub handin: Option<(u64, crate::handoff_io::Listener)>,
    pub next_handin: u64,
    recovery: Recovery,
    published: Vec<u8>,
    pub closed: u32,
    pub fallback: u32,
    epoch: Instant,
    last_frame: i64,
    next_directory: i64,
    pub upgrade_watch: Option<crate::upgrade_io::Watch>,
}

#[derive(Clone, Copy)]
enum Source {
    Listener,
    Signals,
    Client(ClientId),
    Terminal(u32),
    Handin(u64),
    Upgrade,
}

impl Service {
    pub fn now(&self) -> i64 {
        (self.epoch.elapsed().as_nanos().min((i64::MAX - 1) as u128) as i64) + 1
    }
    pub fn index(&self, id: u32) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id == id)
    }
    pub fn new_id(&mut self) -> io::Result<u32> {
        let id = self.next_id;
        self.next_id = id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("workspace tab IDs exhausted"))?;
        Ok(id)
    }

    pub fn queue(&mut self, client: ClientId, kind: u32, tab: u32, body: impl AsRef<[u8]>) {
        if self
            .clients
            .get_mut(&client)
            .is_some_and(|c| c.queue(kind, tab, body.as_ref()).is_err())
        {
            self.drop_client(client);
        }
    }

    pub fn notices(&mut self, tab: u32, notices: Vec<Notice>) {
        for notice in notices {
            match notice {
                Notice::Packet(client, kind, body) => self.queue(client, kind, tab, body),
                Notice::Diagnostic(bytes) => process::diagnostic(&String::from_utf8_lossy(&bytes)),
            }
        }
    }

    pub fn release(&mut self, index: usize) {
        self.tabs[index].release();
    }

    pub fn take_control(&mut self, index: usize, client: ClientId) {
        if self.tabs[index].controller != Some(client) {
            let previous = self.tabs[index].controller;
            self.release(index);
            if self.clients.contains_key(&client) {
                self.tabs[index].controller = Some(client);
            }
            if let Some(previous) = previous {
                self.queue(previous, WIRE_RELEASE, self.tabs[index].id, Vec::new());
            }
        }
    }

    pub fn drop_client(&mut self, client: ClientId) {
        self.clients.remove(&client);
        self.pending_replace.remove(&client);
        for tab in &mut self.tabs {
            if tab.controller == Some(client) {
                tab.release();
            }
            if tab.print_client == Some(client) {
                tab.print_client = None;
            }
        }
    }

    pub fn close(&mut self, index: usize) {
        self.closed = self.tabs[index].id;
        self.pending_replace.retain(|_, (id, _)| *id != self.closed);
        self.fallback = self
            .tabs
            .get(index + 1)
            .or_else(|| index.checked_sub(1).map(|i| &self.tabs[i]))
            .map_or(0, |tab| tab.id);
        self.release(index);
        self.tabs.remove(index);
    }

    pub fn send_frame(&mut self, index: usize, full: bool, now: i64) -> io::Result<bool> {
        let tab = &mut self.tabs[index];
        let Some(terminal) = &mut tab.terminal else {
            return Ok(false);
        };
        let Some(client_id) = tab.controller else {
            if core(terminal.engine.frame(now))?.redraw_deadline == 0 {
                core(terminal.engine.clean())?;
                tab.pending_frame = false;
            }
            return Ok(false);
        };
        let Some(client) = self.clients.get_mut(&client_id) else {
            tab.release();
            return Ok(false);
        };
        if !client.ready {
            return Ok(false);
        }
        if client.needs_resync || client.output.len() >= WORKSPACE_FRAME_BACKLOG_BYTES as usize {
            // The baseline still describes ordered queued bytes. Retain damage for one later delta,
            // but remember explicit full-frame requests across the congestion gate.
            if full {
                tab.screen.bytes.clear();
            }
            client.needs_resync = true;
            tab.pending_frame = false;
            return Ok(false);
        }
        let result = tab.screen.replicate(
            &mut terminal.engine,
            &mut client.output,
            tab.id,
            full,
            client.features & WIRE_FEATURE_COPY_ROWS != 0,
            tab.frame_reply,
            now,
        );
        match result {
            Ok(Outcome::Held) => {
                tab.pending_frame = true;
                Ok(false)
            }
            Ok(outcome) => {
                tab.pending_frame = false;
                if outcome == Outcome::Sent {
                    tab.frame_reply = false;
                }
                Ok(outcome == Outcome::Sent)
            }
            Err(error) => {
                process::diagnostic(&format!("Client frame queue: {error}"));
                self.drop_client(client_id);
                Ok(false)
            }
        }
    }

    fn catalog(&mut self, now: i64) -> io::Result<Vec<u8>> {
        let mut bytes = Buffer::with_limit(usize::MAX);
        for value in [self.tabs.len() as u32, self.closed, self.fallback] {
            bytes.put_u32(value)?;
        }
        for tab in &mut self.tabs {
            bytes.put_bytes(tab.metadata(now)?)?;
        }
        Ok(bytes.into_bytes())
    }

    pub fn send_catalog(&mut self, client: ClientId, now: i64) -> io::Result<()> {
        for index in 0..self.tabs.len() {
            if self.tabs[index].pending_frame {
                self.send_frame(index, false, now)?;
            }
        }
        let bytes = self.catalog(now)?;
        self.queue(client, WIRE_CATALOG, 0, bytes);
        Ok(())
    }

    fn save(&mut self) {
        let home = std::env::var_os("HOME")
            .unwrap_or_else(|| "/".into())
            .into_vec();
        let recipes: Vec<_> = self
            .tabs
            .iter()
            .map(|tab| {
                Recipe::for_tab(
                    &tab.directory,
                    tab.url.as_deref().unwrap_or_default(),
                    &home,
                )
            })
            .collect();
        if let Err(error) = self.recovery.save(&recipes) {
            process::diagnostic(&format!("Workspace recovery save: {error}"));
        }
    }

    pub fn publish(&mut self, now: i64) -> io::Result<()> {
        let mut bytes = self.catalog(now)?;
        if bytes == self.published {
            return Ok(());
        }
        // Corresponding frames precede changed modes/titles. A gated tab retains its prior catalog entry.
        for index in 0..self.tabs.len() {
            if self.tabs[index].pending_frame {
                self.send_frame(index, false, now)?;
            }
        }
        let clients: Vec<_> = self
            .clients
            .values()
            .filter(|client| client.ready)
            .map(|c| c.id)
            .collect();
        for client in clients {
            self.queue(client, WIRE_CATALOG, 0, &bytes);
        }
        bytes[4..12].fill(0);
        self.published = bytes;
        self.closed = 0;
        self.fallback = 0;
        self.save();
        Ok(())
    }

    pub fn accept(&mut self) -> io::Result<()> {
        // Bound accepts so a stream of new connections cannot starve established tabs.
        for _ in 0..32 {
            let socket = match self.listener.accept() {
                Ok((socket, _)) => socket,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            let Ok(pid) = process::peer_pid(socket.as_raw_fd()) else {
                continue;
            };
            socket.set_nonblocking(true)?;
            let id = self.next_client;
            self.next_client = id
                .checked_add(1)
                .ok_or_else(|| io::Error::other("client IDs exhausted"))?;
            self.clients.insert(
                id,
                Client {
                    id,
                    socket,
                    pid,
                    ready: false,
                    features: 0,
                    decoder: Decoder::default(),
                    output: Queue::new(CLIENT_QUEUE),
                    needs_resync: false,
                    selection: None,
                },
            );
        }
        Ok(())
    }

    fn wait(&mut self, signals: &Signals) -> io::Result<(Vec<Source>, Vec<reactor::Ready>)> {
        let mut interests = vec![
            Interest {
                fd: self.listener.as_raw_fd(),
                read: true,
                write: false,
            },
            Interest {
                fd: signals.fd(),
                read: true,
                write: false,
            },
        ];
        let mut sources = vec![Source::Listener, Source::Signals];
        if let Some(watch) = &self.upgrade_watch {
            interests.push(Interest {
                fd: watch.as_raw_fd(),
                read: true,
                write: false,
            });
            sources.push(Source::Upgrade);
        }
        if let Some((id, listener)) = &self.handin {
            interests.push(Interest {
                fd: listener.as_raw_fd(),
                read: true,
                write: false,
            });
            sources.push(Source::Handin(*id));
        }
        for (&id, client) in &self.clients {
            interests.push(Interest {
                fd: client.socket.as_raw_fd(),
                read: true,
                write: !client.output.is_empty(),
            });
            sources.push(Source::Client(id));
        }
        let now = self.now();
        let mut deadline = None;
        for tab in &mut self.tabs {
            if let Some(terminal) = &mut tab.terminal {
                if let Some(fd) = terminal.pty.fd() {
                    interests.push(Interest {
                        fd,
                        read: true,
                        write: !terminal.input.is_empty(),
                    });
                    sources.push(Source::Terminal(tab.id));
                }
                let frame = core(terminal.engine.frame(now))?;
                let due = if frame.redraw_deadline != 0 {
                    Some(frame.redraw_deadline)
                } else {
                    tab.pending_frame
                        .then_some(self.last_frame + WORKSPACE_FRAME_INTERVAL_NS as i64)
                };
                if let Some(due) = due {
                    deadline = Some(deadline.map_or(due, |old: i64| old.min(due)));
                }
            }
        }
        if self.tabs.iter().any(|tab| tab.terminal.is_some()) {
            deadline =
                Some(deadline.map_or(self.next_directory, |old| old.min(self.next_directory)));
        }
        Ok((
            sources,
            reactor::wait(&interests, deadline.map(|due| (due - now).max(0)))?,
        ))
    }

    fn maintain(&mut self) -> io::Result<()> {
        self.reaper.reap();
        let now = self.now();
        if now >= self.next_directory {
            for tab in &mut self.tabs {
                tab.refresh_directory(now);
            }
            self.next_directory = now + WORKSPACE_DIRECTORY_INTERVAL_NS as i64;
        }
        let mut index = 0;
        while index < self.tabs.len() {
            if self.tabs[index]
                .terminal
                .as_ref()
                .is_some_and(|t| t.pty.fd().is_none())
            {
                self.close(index);
            } else {
                index += 1;
            }
        }
        let mut emitted = false;
        if self.last_frame == 0 || now - self.last_frame >= WORKSPACE_FRAME_INTERVAL_NS as i64 {
            for index in 0..self.tabs.len() {
                if self.tabs[index].pending_frame && self.send_frame(index, false, now)? {
                    self.tabs[index].input_frame = false;
                    emitted = true;
                }
            }
        }
        if emitted {
            self.last_frame = now;
        }
        if self.closed != 0
            || self
                .tabs
                .iter()
                .any(|tab| tab.catalog_dirty || tab.catalog_pending)
            || self.published.is_empty()
        {
            self.publish(now)?;
        }
        self.resync(now)
    }

    fn run(&mut self, signals: Signals) -> io::Result<()> {
        process::diagnostic(&format!(
            "Workspace service started (protocol {WIRE_VERSION})"
        ));
        match crate::upgrade_io::Watch::new() {
            Ok(watch) => self.upgrade_watch = Some(watch),
            Err(error) => {
                process::diagnostic(&format!("Automatic upgrade watch unavailable: {error}"))
            }
        }
        while self.running {
            if signals.stopping() {
                break;
            }
            if self.try_upgrade() {
                // Ownership is already committed. Never broadcast or checkpoint the emptied donor.
                return Ok(());
            }
            self.maintain()?;
            let (sources, ready) = self.wait(&signals)?;
            for ready in ready {
                // An earlier callback can close/reuse an FD. Stable IDs reject obsolete readiness.
                match sources[ready.index] {
                    Source::Listener => self.accept()?,
                    Source::Signals => {
                        self.running &= !signals.shutdown_requested()?;
                        self.reaper.reap();
                    }
                    Source::Client(id) => self.client_ready(id, ready.read, ready.write)?,
                    Source::Terminal(id) => self.terminal_ready(id, ready.read, ready.write)?,
                    Source::Handin(id) => self.receive_handoff(id, &signals),
                    Source::Upgrade => {} // The next loop drains the watch before any state publication.
                }
                if !self.running || signals.stopping() {
                    break;
                }
            }
        }
        self.save();
        process::diagnostic("Workspace service stopped");
        Ok(())
    }
}

fn create(scope: &[u8], listener: UnixListener) -> io::Result<(Service, io::Result<Vec<Recipe>>)> {
    listener.set_nonblocking(true)?;
    let home = std::env::var_os("HOME").unwrap_or_else(|| "/".into());
    let root = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| PathBuf::from(home).join(".local/state"));
    let (recovery, recipes) = Recovery::open(recovery::path(&root, scope)?);
    let service = Service {
        listener,
        clients: BTreeMap::new(),
        tabs: Vec::new(),
        next_id: 1,
        next_client: 1,
        reaper: ChildReaper::default(),
        running: true,
        restored_launch: false,
        pending_replace: BTreeMap::new(),
        handin: None,
        next_handin: 1,
        recovery,
        published: Vec::new(),
        closed: 0,
        fallback: 0,
        epoch: Instant::now(),
        last_frame: 0,
        next_directory: 0,
        upgrade_watch: None,
    };
    Ok((service, recipes))
}

pub fn serve(scope: &[u8]) -> io::Result<()> {
    let signals = Signals::new()?;
    let (mut service, recipes) = create(scope, socket::listen(process::uid(), scope)?)?;
    match recipes {
        Ok(recipes) => {
            for recipe in recipes {
                let id = service.new_id()?;
                let tab = if recipe.url.is_empty() {
                    Tab::shell(
                        id,
                        &Launch::restore(process::restore_directory(&recipe.directory)),
                        true,
                        None,
                        service.reaper.clone(),
                    )?
                } else {
                    Tab::browser(id, recipe.url)
                };
                service.tabs.push(tab);
            }
            service.restored_launch = !service.tabs.is_empty();
        }
        Err(error) => process::diagnostic(&format!("Workspace recovery read: {error}")),
    }
    service.run(signals)
}

pub(crate) fn receive_upgrade(scope: &[u8], legacy: bool) -> io::Result<()> {
    let (listener, receiving) = crate::upgrade_io::inherited_listeners()?;
    let manifest = legacy
        .then(crate::upgrade_io::Manifest::inherited)
        .transpose()?;
    // Mandatory signal setup must succeed before any C can transfer authority. A post-commit
    // signalfd failure would otherwise drop the imported PTYs after the donor had disowned them.
    let signals = Signals::new()?;
    let (mut service, recipes) = create(scope, listener)?;
    if let Err(error) = recipes {
        process::diagnostic(&format!("Workspace recovery read: {error}"));
    }
    // A replacement imports live ownership, never recipe-based shells, even if recipes already exist.
    crate::upgrade_io::wait_receiver(&receiving)?;
    let incoming = match &manifest {
        None => crate::handoff::Incoming::Upgrade,
        Some(manifest) => crate::handoff::Incoming::Mapped {
            ids: &manifest.ids,
            next_id: manifest.next_id,
            retiring_pid: manifest.retiring_pid,
        },
    };
    crate::handoff::receive(
        receiving.accept()?,
        &mut service.tabs,
        &mut service.next_id,
        &service.reaper,
        incoming,
        &signals,
    )?;
    drop(manifest);
    drop(receiving);
    service.run(signals)
}

#[cfg(test)]
#[path = "network_tests.rs"]
mod network_tests;
