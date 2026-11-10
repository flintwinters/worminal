//! Native Rust terminal application over the existing v2 workspace. Closing views leaves shells running.
use crate::{
    fonts::Fonts,
    paint::Palette,
    terminal::{Action, Terminal},
};
use gtk::{gdk, prelude::*};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    ffi::{OsStr, OsString},
    io::{self, Read, Write},
    os::{fd::AsRawFd, unix::net::UnixStream},
    path::PathBuf,
    rc::Rc,
    thread,
    time::{Duration, Instant},
};
use worminal_protocol::*;
use worminal_workspace::socket;

#[path = "app_browser.rs"]
mod browser_views;
#[path = "app_chrome.rs"]
mod chrome;
#[path = "app_shortcuts.rs"]
mod shortcuts;

thread_local! { static WINDOWS: RefCell<Vec<Rc<App>>> = const { RefCell::new(Vec::new()) }; }
use config::Config;
#[path = "app_broker.rs"]
mod broker;
#[path = "app_config.rs"]
mod config;
#[path = "app_window_state.rs"]
mod window_state;

fn scope() -> Vec<u8> {
    std::env::var_os("WORMINAL_SHARED_SOCKET_SCOPE")
        .unwrap_or_default()
        .into_encoded_bytes()
}
fn uid() -> io::Result<u32> {
    // Socket scopes use the real UID, matching the C clients.
    let status = std::fs::read_to_string("/proc/self/status")?;
    status
        .lines()
        .find(|line| line.starts_with("Uid:"))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| io::Error::other("cannot read user identity"))
}
fn connect() -> io::Result<UnixStream> {
    socket::connect(uid()?, &scope())
}
fn ensure() -> io::Result<()> {
    if connect().is_ok() {
        return Ok(());
    }
    let daemon = std::env::current_exe()?.with_file_name("worminald");
    // posix_spawn is safe after GTK initialization; subsequent windows/reconnects only connect.
    let child = crate::startup::daemon(&daemon)?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if connect().is_ok() {
            return Ok(());
        }
        match child.try_recv() {
            Ok(result) => {
                result?;
                return connect().map(|_| ());
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => return connect().map(|_| ()),
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
        if Instant::now() >= deadline {
            // A slow service may already own restored shells. Leave it running instead of killing unknown live state.
            return Err(io::Error::other("daemon startup timed out"));
        }
        thread::sleep(Duration::from_millis(10));
    }
}
struct Connection {
    socket: UnixStream,
    _ssh: Option<worminal_workspace::ssh::Child>,
    decoder: Decoder,
    output: VecDeque<Vec<u8>>,
    offset: usize,
    bytes: usize,
}
impl Connection {
    fn new(master: Option<&OsStr>) -> io::Result<Self> {
        let (socket, ssh) = if let Some(master) = master {
            let (socket, child) = worminal_workspace::ssh::connect(master)?;
            (socket, Some(child))
        } else {
            (connect()?, None)
        };
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            _ssh: ssh,
            decoder: Decoder::default(),
            output: VecDeque::new(),
            offset: 0,
            bytes: 0,
        })
    }
    fn queue(&mut self, packet: Packet) -> io::Result<()> {
        let bytes = packet.encode()?;
        if bytes.len() > (WORKSPACE_CLIENT_QUEUE_BYTES as usize).saturating_sub(self.bytes) {
            return Err(io::Error::other("frontend output queue is full"));
        }
        self.bytes += bytes.len();
        self.output.push_back(bytes);
        self.flush()
    }
    fn flush(&mut self) -> io::Result<()> {
        let mut budget = WORKSPACE_IO_BYTES as usize;
        while let Some(front) = self.output.front() {
            if budget == 0 {
                break;
            }
            match self
                .socket
                .write(&front[self.offset..front.len().min(self.offset + budget)])
            {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(count) => {
                    self.offset += count;
                    self.bytes -= count;
                    budget -= count;
                    if self.offset == front.len() {
                        self.output.pop_front();
                        self.offset = 0;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
    fn read(&mut self) -> io::Result<Vec<Packet>> {
        let mut result = Vec::new();
        let mut bytes = [0; WORKSPACE_IO_BYTES as usize];
        for _ in 0..4 {
            match self.socket.read(&mut bytes) {
                Ok(0) => {
                    if result.is_empty() {
                        return Err(io::ErrorKind::UnexpectedEof.into());
                    }
                    break;
                }
                Ok(count) => {
                    let mut input = &bytes[..count];
                    while !input.is_empty() {
                        let (used, packet) = self.decoder.push(input)?;
                        input = &input[used..];
                        if let Some(packet) = packet {
                            result.push(packet);
                        }
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(result)
    }
}
/// Tab ids are global to the window: the high byte names the link that owns the tab, so ids from
/// different daemons never collide. Link 0 is the window's own connection; wire ids stay below `1 << LINK_SHIFT`.
const LINK_SHIFT: u32 = 24;
/// One daemon connection of a window plus the handshake state that belongs to it.
struct Link {
    connection: RefCell<Option<Connection>>,
    reader: RefCell<Option<glib::SourceId>>,
    writer: RefCell<Option<glib::SourceId>>,
    reconnect: RefCell<Option<glib::SourceId>>,
    initial_claim: Cell<bool>,
    pending_tabs: Cell<u32>,
    requested: Cell<u32>,
    features: Cell<u32>,
}
impl Link {
    fn new() -> Rc<Self> {
        Rc::new(Self {
            connection: RefCell::new(None),
            reader: RefCell::new(None),
            writer: RefCell::new(None),
            reconnect: RefCell::new(None),
            initial_claim: Cell::new(true),
            pending_tabs: Cell::new(0),
            requested: Cell::new(0),
            features: Cell::new(0),
        })
    }
    fn clear(&self) {
        for source in [&self.reader, &self.writer, &self.reconnect] {
            if let Some(source) = source.borrow_mut().take() {
                source.remove();
            }
        }
        self.connection.borrow_mut().take();
    }
}
/// Another `--master` host whose tabs are shown in this window beside its own.
struct Remote {
    index: u32,
    master: OsString,
    config: Config,
    link: Rc<Link>,
}
#[derive(Clone)]
struct Page {
    id: u32,
    widget: gtk::Widget,
    label: gtk::Label,
    terminal: Option<Terminal>,
    palette: Rc<RefCell<Palette>>,
    title: String,
    directory: PathBuf,
    browser: bool,
    url: String,
    view: Rc<RefCell<Option<crate::browser::Browser>>>,
}
struct Record {
    id: u32,
    browser: bool,
    url: String,
    mode: u32,
    cursor: u32,
    title: String,
    directory: PathBuf,
    colors: Vec<String>,
}
fn text(body: &mut Payload<'_>) -> io::Result<String> {
    Ok(
        String::from_utf8_lossy(body.string()?.split(|byte| *byte == 0).next().unwrap())
            .into_owned(),
    )
}
struct App {
    state_id: u32,
    window: gtk::Window,
    notebook: gtk::Notebook,
    tabstrip: crate::tabs::Tabs,
    status: gtk::Label,
    config: Config,
    fonts: Rc<RefCell<Fonts>>,
    pages: RefCell<Vec<Page>>,
    active: Cell<u32>,
    syncing: Cell<bool>,
    primary: Rc<Link>,
    remotes: RefCell<Vec<Rc<Remote>>>,
    next_link: Cell<u32>,
    closing: Cell<bool>,
    geometry_pending: Cell<bool>,
    early_keys: RefCell<VecDeque<gdk::EventKey>>,
}
impl App {
    fn print(&self, bytes: &[u8]) -> io::Result<()> {
        let mut output = &*self.config.stream;
        output.write_all(bytes)?;
        output.flush()
    }
    /// A forwarded `--master` launch joins the active window instead of opening another.
    fn launch(config: Config) -> io::Result<Rc<Self>> {
        if config.master.is_some() {
            let target = WINDOWS.with(|windows| {
                let windows = windows.borrow();
                windows
                    .iter()
                    .find(|app| !app.closing.get() && app.window.is_active())
                    .or_else(|| windows.iter().rev().find(|app| !app.closing.get()))
                    .cloned()
            });
            if let Some(target) = target {
                target.add_remote(config)?;
                return Ok(target);
            }
        }
        Self::open(config)
    }
    fn open(config: Config) -> io::Result<Rc<Self>> {
        // New launches may arrive in an existing GTK owner after service loss. The shared
        // posix_spawn boundary can recover the service here without GUI-process fork hooks.
        if config.master.is_none() {
            ensure()?;
        }
        if let Some(class) = &config.class {
            gdk::set_program_class(class);
        }
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        if let Some(name) = &config.name {
            window.set_widget_name(name);
        }
        window.set_title(&config.title);
        window.set_default_size(1000, 600);
        window.set_resizable(!config.fixed);
        let notebook = gtk::Notebook::new();
        notebook.set_show_tabs(false);
        notebook.set_show_border(false);
        let fonts = Rc::new(RefCell::new(Fonts::new(config.font.clone(), config.zoom)));
        let tabstrip = crate::tabs::Tabs::new(fonts.clone());
        let status = gtk::Label::new(None);
        status.set_no_show_all(true);
        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        container.pack_start(tabstrip.widget(), false, false, 0);
        container.pack_start(&notebook, true, true, 0);
        container.pack_end(&status, false, false, 0);
        chrome::outline(&window, &container);
        let app = Rc::new(Self {
            state_id: window_state::next_id()?,
            fonts,
            tabstrip,
            config,
            window,
            notebook,
            status,
            pages: RefCell::new(Vec::new()),
            active: Cell::new(0),
            syncing: Cell::new(false),
            primary: Link::new(),
            remotes: RefCell::new(Vec::new()),
            next_link: Cell::new(1),
            closing: Cell::new(false),
            geometry_pending: Cell::new(true),
            early_keys: RefCell::new(VecDeque::new()),
        });
        app.tabstrip.connect_selected(
            glib::clone!(@weak app => move |id| app.select_id(id)),
            glib::clone!(@weak app => move |id| app.send(id, WIRE_CLOSE, Vec::new())),
        );
        // GTK maps the new child in its default switch handler. Focus it afterwards.
        app.notebook.connect_local("switch-page", true, glib::clone!(@weak app => @default-return None, move |values| {
            if app.syncing.get() { return None; }
            let widget = values[1].get::<gtk::Widget>().ok()?;
            let id = app.pages.borrow().iter().find(|page| page.widget == widget).map(|page| page.id);
            if let Some(id) = id { app.activate(id); }
            None
        }));
        app.window.connect_key_press_event(
            glib::clone!(@weak app => @default-return glib::Propagation::Proceed,
            move |_, event| app.shortcut(event).into()),
        );
        app.window.connect_focus_in_event(glib::clone!(@weak app => @default-return glib::Propagation::Proceed,
            move |_, _| {
                if app.pages.borrow().iter().any(|page| page.id == app.active.get() && page.browser) {
                    app.claim_browser(app.active.get());
                }
                glib::Propagation::Proceed
            }));
        app.window.connect_is_active_notify(|window| {
            if window.is_active() {
                window.set_urgency_hint(false);
            }
            window_state::changed();
        });
        app.window.connect_delete_event(
            glib::clone!(@weak app => @default-return glib::Propagation::Proceed, move |_, _| {
                app.release_browsers(); glib::Propagation::Proceed
            }),
        );
        app.window
            .connect_destroy(glib::clone!(@weak app => move |_| {
                app.closing.set(true); app.clear_connection();
                WINDOWS.with(|windows| { let mut windows = windows.borrow_mut();
                    let previous = windows.len();
                    windows.retain(|owner| !Rc::ptr_eq(owner, &app));
                    if windows.len() != previous && windows.is_empty() { gtk::main_quit(); } });
                window_state::changed();
            }));
        if let Err(error) = app.establish(0, !app.config.attach) {
            app.window.close();
            return Err(error);
        }
        WINDOWS.with(|windows| windows.borrow_mut().push(app.clone()));
        window_state::changed();
        crate::graphics_ffi::show_window(&app.window);
        Ok(app)
    }
    fn message(&self, message: Option<&str>) {
        self.status.set_text(message.unwrap_or(""));
        self.status.set_visible(message.is_some());
    }
    fn link(&self, index: u32) -> Option<Rc<Link>> {
        if index == 0 {
            return Some(self.primary.clone());
        }
        self.remotes
            .borrow()
            .iter()
            .find(|remote| remote.index == index)
            .map(|remote| remote.link.clone())
    }
    /// The launch configuration of one link: the window's own, or the one its `--master` was opened with.
    fn config_for(&self, index: u32) -> Config {
        self.remotes
            .borrow()
            .iter()
            .find(|remote| remote.index == index)
            .map_or_else(|| self.config.clone(), |remote| remote.config.clone())
    }
    /// Daemon host of a tab, or `None` for this machine.
    fn master_of(&self, id: u32) -> Option<OsString> {
        self.config_for(id >> LINK_SHIFT).master
    }
    fn features(&self, id: u32) -> u32 {
        self.link(id >> LINK_SHIFT)
            .map_or(0, |link| link.features.get())
    }
    fn establish(self: &Rc<Self>, index: u32, launch: bool) -> io::Result<()> {
        let config = self.config_for(index);
        let link = self
            .link(index)
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        let connection = Connection::new(config.master.as_deref())?;
        *link.connection.borrow_mut() = Some(connection);
        link.initial_claim.set(true);
        self.watch(index, &link);
        self.message(None);
        link.features.set(0);
        self.send_link(
            index,
            0,
            WIRE_CAPABILITIES,
            (WIRE_FEATURE_COPY_ROWS | WIRE_FEATURE_ROW_SPAN | WIRE_FEATURE_FOCUSED_FRAMES)
                .to_be_bytes()
                .to_vec(),
        );
        if launch && index == 0 && config.browser_url.is_some() {
            self.open_browser(config.browser_url.as_deref().unwrap());
        } else if launch {
            self.send_link(index, 0, WIRE_HELLO, config.launch()?);
        } else {
            self.send_link(index, 0, WIRE_ATTACH, Vec::new());
        }
        if link.connection.borrow().is_none() {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        Ok(())
    }
    fn watch(self: &Rc<Self>, index: u32, link: &Rc<Link>) {
        let fd = link
            .connection
            .borrow()
            .as_ref()
            .map(|connection| connection.socket.as_raw_fd());
        let Some(fd) = fd else { return };
        let weak = Rc::downgrade(self);
        let link_weak = Rc::downgrade(link);
        *link.reader.borrow_mut() = Some(glib::source::unix_fd_add_local(
            fd,
            glib::IOCondition::IN | glib::IOCondition::HUP | glib::IOCondition::ERR,
            move |_, _| {
                let (Some(app), Some(link)) = (weak.upgrade(), link_weak.upgrade()) else {
                    return glib::ControlFlow::Break;
                };
                let packets = link.connection.borrow_mut().as_mut().map(Connection::read);
                match packets {
                    Some(Ok(packets)) => {
                        for packet in packets {
                            // A catalog can retire this link while later packets are still queued.
                            if app.closing.get() || link.connection.borrow().is_none() {
                                return glib::ControlFlow::Break;
                            }
                            if let Err(error) = app.packet(index, packet) {
                                app.lost(index, &error.to_string());
                                return glib::ControlFlow::Break;
                            }
                        }
                        glib::ControlFlow::Continue
                    }
                    Some(Err(error)) => {
                        app.lost(index, &error.to_string());
                        glib::ControlFlow::Break
                    }
                    None => glib::ControlFlow::Break,
                }
            },
        ));
    }
    fn clear_connection(&self) {
        self.primary.clear();
        for remote in self.remotes.borrow().iter() {
            remote.link.clear();
        }
    }
    /// Show another daemon's tabs in this window. `--master` hosts already present are only raised.
    fn add_remote(self: &Rc<Self>, config: Config) -> io::Result<()> {
        let master = config.master.clone().expect("remote launch has a master");
        let known = self.config.master.as_ref() == Some(&master)
            || self
                .remotes
                .borrow()
                .iter()
                .any(|remote| remote.master == master);
        if !known {
            let index = self.next_link.get();
            if index >= 1 << (32 - LINK_SHIFT) {
                return Err(io::Error::other("too many remote hosts in one window"));
            }
            self.next_link.set(index + 1);
            let launch = !config.attach;
            self.remotes.borrow_mut().push(Rc::new(Remote {
                index,
                master,
                config,
                link: Link::new(),
            }));
            if let Err(error) = self.establish(index, launch) {
                self.drop_remote(index);
                return Err(error);
            }
        }
        self.window.present();
        Ok(())
    }
    fn drop_remote(&self, index: u32) {
        if let Some(link) = self.link(index).filter(|_| index != 0) {
            link.clear();
        }
        self.remotes
            .borrow_mut()
            .retain(|remote| remote.index != index);
    }
    fn lost(self: &Rc<Self>, index: u32, error: &str) {
        if self.closing.get() {
            return;
        }
        let Some(link) = self.link(index) else { return };
        link.clear();
        link.pending_tabs.set(0);
        for page in self.pages.borrow().iter() {
            if page.id >> LINK_SHIFT == index {
                if let Some(terminal) = &page.terminal {
                    terminal.release();
                }
            }
        }
        self.message(Some(&format!("Disconnected: {error}. Reconnecting…")));
        window_state::changed();
        if link.reconnect.borrow().is_none() {
            let weak = Rc::downgrade(self);
            let link_weak = Rc::downgrade(&link);
            *link.reconnect.borrow_mut() = Some(glib::timeout_add_local(
                Duration::from_millis(250),
                move || {
                    let (Some(app), Some(link)) = (weak.upgrade(), link_weak.upgrade()) else {
                        return glib::ControlFlow::Break;
                    };
                    if app.establish(index, false).is_ok() {
                        link.reconnect.borrow_mut().take();
                        glib::ControlFlow::Break
                    } else {
                        glib::ControlFlow::Continue
                    }
                },
            ));
        }
    }
    fn send(self: &Rc<Self>, tab: u32, kind: u32, body: Vec<u8>) {
        self.send_link(tab >> LINK_SHIFT, tab & ((1 << LINK_SHIFT) - 1), kind, body);
    }
    fn send_link(self: &Rc<Self>, index: u32, tab: u32, kind: u32, body: Vec<u8>) {
        let Some(link) = self.link(index) else { return };
        let result = link
            .connection
            .borrow_mut()
            .as_mut()
            .map(|connection| connection.queue(Packet { kind, tab, body }));
        if let Some(Err(error)) = result {
            self.lost(index, &error.to_string());
            return;
        }
        let fd = link
            .connection
            .borrow()
            .as_ref()
            .filter(|connection| !connection.output.is_empty())
            .map(|connection| connection.socket.as_raw_fd());
        if link.writer.borrow().is_none() {
            if let Some(fd) = fd {
                let weak = Rc::downgrade(self);
                let link_weak = Rc::downgrade(&link);
                *link.writer.borrow_mut() = Some(glib::source::unix_fd_add_local(
                    fd,
                    glib::IOCondition::OUT | glib::IOCondition::HUP | glib::IOCondition::ERR,
                    move |_, _| {
                        let (Some(app), Some(link)) = (weak.upgrade(), link_weak.upgrade()) else {
                            return glib::ControlFlow::Break;
                        };
                        let result = link.connection.borrow_mut().as_mut().map(|connection| {
                            connection.flush().map(|_| connection.output.is_empty())
                        });
                        match result {
                            Some(Ok(false)) => glib::ControlFlow::Continue,
                            Some(Err(error)) => {
                                app.lost(index, &error.to_string());
                                glib::ControlFlow::Break
                            }
                            _ => {
                                link.writer.borrow_mut().take();
                                glib::ControlFlow::Break
                            }
                        }
                    },
                ));
            }
        }
    }
    fn terminal(&self, id: u32) -> Option<Terminal> {
        self.pages
            .borrow()
            .iter()
            .find(|page| page.id == id)
            .and_then(|page| page.terminal.clone())
    }
    fn activate(self: &Rc<Self>, id: u32) {
        self.active.set(id);
        let title = self
            .pages
            .borrow()
            .iter()
            .find(|page| page.id == id)
            .map(|page| page.title.clone());
        if let Some(title) = title {
            self.window.set_title(&title);
        }
        if let Some(terminal) = self.terminal(id) {
            self.fit_geometry(&terminal);
            let (cols, rows) = terminal.dimensions();
            self.send(
                id,
                WIRE_FOCUS,
                [cols, rows]
                    .iter()
                    .flat_map(|value| value.to_be_bytes())
                    .collect(),
            );
            terminal.select();
            terminal.widget().grab_focus();
        } else {
            self.claim_browser(id);
        }
        self.refresh_tabs();
        window_state::changed();
    }
    fn fit_geometry(&self, terminal: &Terminal) {
        if !self.geometry_pending.get() || !self.window.is_mapped() {
            return;
        }
        if let Some(metrics) = terminal.metrics() {
            self.geometry_pending.set(false);
            let (width, height) = self.window.size();
            self.window.resize(
                self.config.columns as i32 * metrics.width + width
                    - terminal.widget().allocated_width()
                    + 4,
                self.config.rows as i32 * metrics.height + height
                    - terminal.widget().allocated_height()
                    + 4,
            );
        }
    }
    fn action(self: &Rc<Self>, id: u32, action: Action) {
        match action {
            Action::Send(kind, bytes) => {
                // Keys, paste and mouse reports answer a bell; focus reports use WIRE_FOCUS_STATE.
                if kind == WIRE_INPUT {
                    self.tabstrip.answer(id);
                }
                self.send(id, kind, bytes)
            }
            Action::Claim => self.activate(id),
            Action::Resized(cols, rows) if !self.syncing.get() && self.active.get() == id => {
                if let Some(terminal) = self.terminal(id) {
                    self.fit_geometry(&terminal);
                }
                if self
                    .terminal(id)
                    .is_some_and(|terminal| terminal.live() || self.window.is_active())
                {
                    self.send(
                        id,
                        WIRE_RESIZE,
                        [cols, rows]
                            .iter()
                            .flat_map(|value| value.to_be_bytes())
                            .collect(),
                    );
                }
            }
            Action::Print(bytes) => {
                if let Err(error) = self.print(&bytes) {
                    self.message(Some(&error.to_string()));
                }
            }
            Action::Error(message) => self.message(Some(&message)),
            _ => {}
        }
    }
    fn packet(self: &Rc<Self>, index: u32, mut packet: Packet) -> io::Result<()> {
        let Some(link) = self.link(index) else {
            return Ok(());
        };
        if packet.tab >= 1 << LINK_SHIFT {
            return Err(io::Error::other("invalid tab id"));
        }
        if packet.tab != 0 {
            packet.tab |= index << LINK_SHIFT;
        }
        match packet.kind {
            WIRE_CAPABILITIES => {
                let mut body = Payload::new(&packet.body);
                link.features.set(body.u32()?);
                body.finish()?;
            }
            WIRE_CATALOG => self.catalog(index, &packet.body)?,
            WIRE_CREATED => {
                link.requested.set(packet.tab);
                link.pending_tabs
                    .set(link.pending_tabs.get().saturating_sub(1));
            }
            WIRE_ERROR => self.message(Some(&text(&mut Payload::new(&packet.body))?)),
            WIRE_RELEASE => {
                if let Some(terminal) = self.terminal(packet.tab) {
                    terminal.release();
                }
            }
            WIRE_BELL => {
                self.window.error_bell();
                self.tabstrip.alert(packet.tab);
                if !self.window.is_active() {
                    self.window.set_urgency_hint(true);
                }
            }
            WIRE_PRINT => {
                self.print(&packet.body)?;
            }
            WIRE_CLIPBOARD => {
                if self
                    .terminal(packet.tab)
                    .is_some_and(|terminal| terminal.live())
                {
                    gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD)
                        .set_text(&text(&mut Payload::new(&packet.body))?);
                }
            }
            _ => {
                if let Some(terminal) = self.terminal(packet.tab) {
                    terminal.packet(&packet)?;
                }
            }
        }
        Ok(())
    }
    #[expect(
        clippy::too_many_lines,
        reason = "catalog import applies tab, focus and title changes in one pass"
    )]
    fn catalog(self: &Rc<Self>, index: u32, bytes: &[u8]) -> io::Result<()> {
        let link = self
            .link(index)
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        let mut body = Payload::new(bytes);
        let count = body.u32()?;
        if count > 4096 {
            return Err(io::Error::other("invalid catalog count"));
        }
        let closed = body.u32()?;
        let fallback = body.u32()?;
        let mut records = Vec::new();
        for _ in 0..count {
            let id = body.u32()?;
            let kind = body.u32()?;
            let wire_id = id;
            let id = id | index << LINK_SHIFT;
            let url = text(&mut body)?;
            let mode = body.u32()?;
            let cursor = body.u32()?;
            let title = text(&mut body)?;
            let directory = {
                // Filesystem paths stay as protocol bytes; only their painted labels use lossy Unicode.
                // New-shell cwd and the public D-Bus byte array must retain the original filename.
                use std::os::unix::ffi::OsStringExt;
                let bytes = body.string()?.split(|byte| *byte == 0).next().unwrap();
                PathBuf::from(std::ffi::OsString::from_vec(bytes.to_vec()))
            };
            let colors = body.u32()?;
            if wire_id == 0
                || wire_id >= 1 << LINK_SHIFT
                || kind > 1
                || colors > worminal_config::COLOR_NAMES.len() as u32
                || records.iter().any(|record: &Record| record.id == id)
            {
                return Err(io::Error::other("invalid catalog entry"));
            }
            let mut palette = Vec::new();
            for _ in 0..colors {
                palette.push(text(&mut body)?);
            }
            // Browser views stay with the window's own daemon.
            if index != 0 && kind != 0 {
                continue;
            }
            records.push(Record {
                id,
                browser: kind != 0,
                url,
                mode,
                cursor,
                title,
                directory,
                colors: palette,
            });
        }
        body.finish()?;
        let initial_claim = link.initial_claim.replace(false);
        let mut replaced_active = false;
        self.syncing.set(true);
        let obsolete: Vec<_> = self
            .pages
            .borrow()
            .iter()
            .filter(|page| {
                page.id >> LINK_SHIFT == index
                    && !records
                        .iter()
                        .any(|record| record.id == page.id && record.browser == page.browser)
            })
            .cloned()
            .collect();
        for page in obsolete {
            replaced_active |= page.id == self.active.get();
            self.pages
                .borrow_mut()
                .retain(|current| current.id != page.id);
            if let Some(index) = self.notebook.page_num(&page.widget) {
                self.notebook.remove_page(Some(index));
            }
        }
        for record in &records {
            let existing = self
                .pages
                .borrow()
                .iter()
                .find(|page| page.id == record.id)
                .cloned();
            let page = if let Some(page) = existing {
                page
            } else {
                let palette = Rc::new(RefCell::new(Palette::default()));
                let terminal = if record.browser {
                    None
                } else {
                    let weak = Rc::downgrade(self);
                    let id = record.id;
                    Some(Terminal::new(
                        self.config.columns,
                        self.config.rows,
                        self.fonts.clone(),
                        palette.clone(),
                        move |action| {
                            if let Some(app) = weak.upgrade() {
                                app.action(id, action);
                            }
                        },
                    )?)
                };
                let widget: gtk::Widget = if let Some(terminal) = &terminal {
                    terminal.widget().clone().upcast()
                } else {
                    let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
                    let claim = gtk::Button::with_label("Show browser here");
                    let id = record.id;
                    claim.connect_clicked(
                        glib::clone!(@weak self as app => move |_| app.claim_browser(id)),
                    );
                    container.pack_start(&claim, true, true, 0);
                    container.upcast()
                };
                let page = Page {
                    id: record.id,
                    widget,
                    label: gtk::Label::new(None),
                    terminal,
                    palette,
                    title: String::new(),
                    directory: PathBuf::new(),
                    browser: record.browser,
                    url: record.url.clone(),
                    view: Rc::new(RefCell::new(None)),
                };
                self.pages.borrow_mut().push(page.clone());
                self.notebook.append_page(&page.widget, Some(&page.label));
                page.widget.show_all();
                page.label.show();
                page
            };
            let mut palette_changed = false;
            // The catalog lists overrides only through the last one; later entries are defaults.
            for index in 0..worminal_config::COLOR_NAMES.len() {
                let color = record.colors.get(index).filter(|color| !color.is_empty());
                palette_changed |= page
                    .palette
                    .borrow_mut()
                    .set(index, color.map(String::as_str));
            }
            if let Some(terminal) = &page.terminal {
                terminal.configure(record.mode, record.cursor, palette_changed);
            }
            if record.browser {
                if palette_changed {
                    if let Some(view) = page.view.borrow().as_ref() {
                        view.background(page.palette.borrow().color(259));
                    }
                }
                self.browser_metadata(record);
            }
            page.widget.set_tooltip_text(if record.browser {
                Some(&record.url)
            } else {
                None
            });
            if let Some(page) = self
                .pages
                .borrow_mut()
                .iter_mut()
                .find(|page| page.id == record.id)
            {
                if !record.browser || page.title.is_empty() {
                    page.title = record.title.clone();
                }
                page.directory = record.directory.clone();
            }
        }
        {
            let mut pages = self.pages.borrow_mut();
            // Each daemon's catalog owns the order of its tabs; links stay grouped in connection order.
            // Update the collection before painting or hit testing it.
            let order: std::collections::HashMap<_, _> = records
                .iter()
                .enumerate()
                .map(|(position, record)| (record.id, position))
                .collect();
            pages.sort_by_key(|page| (page.id >> LINK_SHIFT, order.get(&page.id).copied()));
            for (position, page) in pages.iter().enumerate() {
                self.notebook
                    .reorder_child(&page.widget, Some(position as u32));
            }
        }
        self.syncing.set(false);
        let requested = link.requested.replace(0);
        let active = self.active.get();
        let fallback = fallback | index << LINK_SHIFT;
        let mut chosen = if requested != 0 {
            requested
        } else if records.iter().any(|record| record.id == active) {
            active
        } else if active >> LINK_SHIFT == index || active == 0 || (initial_claim && index != 0) {
            // Only this link's own tab losing focus, or a newly opened host, moves the selection.
            if records.iter().any(|record| record.id == fallback) {
                fallback
            } else {
                records.first().map_or(0, |record| record.id)
            }
        } else {
            active
        };
        if chosen == 0 {
            chosen = self.pages.borrow().first().map_or(0, |page| page.id);
        }
        let page = self
            .pages
            .borrow()
            .iter()
            .find(|page| page.id == chosen)
            .cloned();
        if let Some(page) = page {
            self.message(None);
            // Reclaim terminal controllers after reconnect, but preserve the existing local browser owner.
            // A mirror must not pull its live DOM into another window just because its socket was replaced.
            let changed = self.active.get() != chosen
                || requested != 0
                || replaced_active
                || (initial_claim
                    && page.id >> LINK_SHIFT == index
                    && (!page.browser || page.view.borrow().is_some()));
            self.syncing.set(true);
            self.notebook
                .set_current_page(self.notebook.page_num(&page.widget));
            self.syncing.set(false);
            if changed {
                self.activate(chosen);
            } else {
                self.window.set_title(&page.title);
            }
        } else if closed != 0 && index == 0 {
            self.window.close();
        } else {
            self.message(Some("No tabs. Ctrl+Shift+T opens a terminal."));
        }
        if index != 0 && records.is_empty() {
            self.drop_remote(index);
        }
        let early: Vec<_> = self.early_keys.borrow_mut().drain(..).collect();
        for event in early {
            if self.closing.get() {
                break;
            }
            if !self.shortcut(&event) {
                if let Some(terminal) = self.terminal(self.active.get()) {
                    terminal.key(&event);
                }
            }
        }
        self.refresh_tabs();
        window_state::changed();
        Ok(())
    }
    fn new_tab(self: &Rc<Self>) {
        // A new tab opens on the host of the tab being viewed.
        let index = self.active.get() >> LINK_SHIFT;
        let mut config = self.config_for(index);
        config.command.clear();
        config.title = "Worminal".into();
        config.output.clear();
        config.line.clear();
        config.browser_url = None;
        if let Some(page) = self
            .pages
            .borrow()
            .iter()
            .find(|page| page.id == self.active.get())
        {
            if !page.directory.as_os_str().is_empty() {
                config.cwd = page.directory.clone();
            }
        }
        match config.launch() {
            Ok(bytes) => {
                if let Some(link) = self.link(index) {
                    link.pending_tabs
                        .set(link.pending_tabs.get().saturating_add(1));
                }
                self.send_link(index, 0, WIRE_NEW, bytes);
            }
            Err(error) => self.message(Some(&error.to_string())),
        }
    }
}
pub fn run() -> io::Result<()> {
    let arguments: Vec<_> = std::env::args_os().collect();
    run_with_listener(arguments, None, true)
}
pub(crate) fn run_with_listener(
    arguments: Vec<std::ffi::OsString>,
    listener: Option<std::os::unix::net::UnixListener>,
    candidate: bool,
) -> io::Result<()> {
    worminal_workspace::browser_target::command(&arguments)?;
    let request = worminal_workspace::broker::Request::capture(arguments)?;
    let Some(config) = Config::parse(&request, candidate)? else {
        return Ok(());
    };
    crate::browser::configure_profile(candidate)?;
    let listener = match listener {
        Some(listener) => listener,
        None => match broker::forward(&request)? {
            Some(listener) => listener,
            None => return Ok(()),
        },
    };
    glib::set_prgname(Some("worminal"));
    gtk::init().map_err(|error| io::Error::other(format!("cannot open display: {error}")))?;
    let _window_state = window_state::start();
    App::launch(config)?;
    let _broker = broker::serve(listener, candidate)?;
    gtk::main();
    Ok(())
}
