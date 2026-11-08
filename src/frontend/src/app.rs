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
    ffi::OsStr,
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
    requested: Cell<u32>,
    syncing: Cell<bool>,
    connection: RefCell<Option<Connection>>,
    reader: RefCell<Option<glib::SourceId>>,
    writer: RefCell<Option<glib::SourceId>>,
    reconnect: RefCell<Option<glib::SourceId>>,
    closing: Cell<bool>,
    geometry_pending: Cell<bool>,
    early_keys: RefCell<VecDeque<gdk::EventKey>>,
    initial_claim: Cell<bool>,
    pending_tabs: Cell<u32>,
    service_features: Cell<u32>,
}
impl App {
    fn print(&self, bytes: &[u8]) -> io::Result<()> {
        let mut output = &*self.config.stream;
        output.write_all(bytes)?;
        output.flush()
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
            requested: Cell::new(0),
            syncing: Cell::new(false),
            connection: RefCell::new(None),
            reader: RefCell::new(None),
            writer: RefCell::new(None),
            reconnect: RefCell::new(None),
            closing: Cell::new(false),
            geometry_pending: Cell::new(true),
            early_keys: RefCell::new(VecDeque::new()),
            initial_claim: Cell::new(true),
            pending_tabs: Cell::new(0),
            service_features: Cell::new(0),
        });
        app.tabstrip
            .connect_selected(glib::clone!(@weak app => move |id| app.select_id(id)));
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
                if let Some(source) = app.reconnect.borrow_mut().take() { source.remove(); }
                WINDOWS.with(|windows| { let mut windows = windows.borrow_mut();
                    let previous = windows.len();
                    windows.retain(|owner| !Rc::ptr_eq(owner, &app));
                    if windows.len() != previous && windows.is_empty() { gtk::main_quit(); } });
                window_state::changed();
            }));
        if let Err(error) = app.establish(!app.config.attach) {
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
    fn establish(self: &Rc<Self>, launch: bool) -> io::Result<()> {
        let connection = Connection::new(self.config.master.as_deref())?;
        let fd = connection.socket.as_raw_fd();
        *self.connection.borrow_mut() = Some(connection);
        self.initial_claim.set(true);
        let weak = Rc::downgrade(self);
        *self.reader.borrow_mut() = Some(glib::source::unix_fd_add_local(
            fd,
            glib::IOCondition::IN | glib::IOCondition::HUP | glib::IOCondition::ERR,
            move |_, _| {
                let Some(app) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                let packets = app.connection.borrow_mut().as_mut().map(Connection::read);
                match packets {
                    Some(Ok(packets)) => {
                        for packet in packets {
                            if app.closing.get() {
                                return glib::ControlFlow::Break;
                            }
                            if let Err(error) = app.packet(packet) {
                                app.lost(&error.to_string());
                                return glib::ControlFlow::Break;
                            }
                        }
                        glib::ControlFlow::Continue
                    }
                    Some(Err(error)) => {
                        app.lost(&error.to_string());
                        glib::ControlFlow::Break
                    }
                    None => glib::ControlFlow::Break,
                }
            },
        ));
        self.message(None);
        self.service_features.set(0);
        self.send(
            0,
            WIRE_CAPABILITIES,
            (WIRE_FEATURE_COPY_ROWS | WIRE_FEATURE_ROW_SPAN).to_be_bytes().to_vec(),
        );
        if launch && self.config.browser_url.is_some() {
            self.open_browser(self.config.browser_url.as_deref().unwrap());
        } else if launch {
            self.send(0, WIRE_HELLO, self.config.launch()?);
        } else {
            self.send(0, WIRE_ATTACH, Vec::new());
        }
        if self.connection.borrow().is_none() {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        Ok(())
    }
    fn clear_connection(&self) {
        if let Some(source) = self.reader.borrow_mut().take() {
            source.remove();
        }
        if let Some(source) = self.writer.borrow_mut().take() {
            source.remove();
        }
        self.connection.borrow_mut().take();
    }
    fn lost(self: &Rc<Self>, error: &str) {
        if self.closing.get() {
            return;
        }
        self.clear_connection();
        self.pending_tabs.set(0);
        for page in self.pages.borrow().iter() {
            if let Some(terminal) = &page.terminal {
                terminal.release();
            }
        }
        self.message(Some(&format!("Disconnected: {error}. Reconnecting…")));
        window_state::changed();
        if self.reconnect.borrow().is_none() {
            let weak = Rc::downgrade(self);
            *self.reconnect.borrow_mut() = Some(glib::timeout_add_local(
                Duration::from_millis(250),
                move || {
                    let Some(app) = weak.upgrade() else {
                        return glib::ControlFlow::Break;
                    };
                    if app.establish(false).is_ok() {
                        app.reconnect.borrow_mut().take();
                        glib::ControlFlow::Break
                    } else {
                        glib::ControlFlow::Continue
                    }
                },
            ));
        }
    }
    fn send(self: &Rc<Self>, tab: u32, kind: u32, body: Vec<u8>) {
        let result = self
            .connection
            .borrow_mut()
            .as_mut()
            .map(|connection| connection.queue(Packet { kind, tab, body }));
        if let Some(Err(error)) = result {
            self.lost(&error.to_string());
            return;
        }
        let fd = self
            .connection
            .borrow()
            .as_ref()
            .filter(|connection| !connection.output.is_empty())
            .map(|connection| connection.socket.as_raw_fd());
        if self.writer.borrow().is_none() {
            if let Some(fd) = fd {
                let weak = Rc::downgrade(self);
                *self.writer.borrow_mut() = Some(glib::source::unix_fd_add_local(
                    fd,
                    glib::IOCondition::OUT | glib::IOCondition::HUP | glib::IOCondition::ERR,
                    move |_, _| {
                        let Some(app) = weak.upgrade() else {
                            return glib::ControlFlow::Break;
                        };
                        let result = app.connection.borrow_mut().as_mut().map(|connection| {
                            connection.flush().map(|_| connection.output.is_empty())
                        });
                        match result {
                            Some(Ok(false)) => glib::ControlFlow::Continue,
                            Some(Err(error)) => {
                                app.lost(&error.to_string());
                                glib::ControlFlow::Break
                            }
                            _ => {
                                app.writer.borrow_mut().take();
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
    fn packet(self: &Rc<Self>, packet: Packet) -> io::Result<()> {
        match packet.kind {
            WIRE_CAPABILITIES => {
                let mut body = Payload::new(&packet.body);
                self.service_features.set(body.u32()?);
                body.finish()?;
            }
            WIRE_CATALOG => self.catalog(&packet.body)?,
            WIRE_CREATED => {
                self.requested.set(packet.tab);
                self.pending_tabs
                    .set(self.pending_tabs.get().saturating_sub(1));
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
    fn catalog(self: &Rc<Self>, bytes: &[u8]) -> io::Result<()> {
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
            if id == 0
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
        let initial_claim = self.initial_claim.replace(false);
        let mut replaced_active = false;
        self.syncing.set(true);
        let obsolete: Vec<_> = self
            .pages
            .borrow()
            .iter()
            .filter(|page| {
                !records
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
        for (index, record) in records.iter().enumerate() {
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
            for (index, color) in record.colors.iter().enumerate() {
                palette_changed |= page
                    .palette
                    .borrow_mut()
                    .set(index, if color.is_empty() { None } else { Some(color) });
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
            self.notebook
                .reorder_child(&page.widget, Some(index as u32));
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
            // The catalog owns tab order; update the collection before painting or hit testing it.
            if !pages
                .iter()
                .map(|page| page.id)
                .eq(records.iter().map(|record| record.id))
            {
                let order: std::collections::HashMap<_, _> = records
                    .iter()
                    .enumerate()
                    .map(|(index, record)| (record.id, index))
                    .collect();
                pages.sort_by_key(|page| order[&page.id]);
            }
        }
        self.syncing.set(false);
        let requested = self.requested.replace(0);
        let chosen = if requested != 0 {
            requested
        } else if records.iter().any(|record| record.id == self.active.get()) {
            self.active.get()
        } else if records.iter().any(|record| record.id == fallback) {
            fallback
        } else {
            records.first().map_or(0, |record| record.id)
        };
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
                || (initial_claim && (!page.browser || page.view.borrow().is_some()));
            self.syncing.set(true);
            self.notebook
                .set_current_page(self.notebook.page_num(&page.widget));
            self.syncing.set(false);
            if changed {
                self.activate(chosen);
            } else {
                self.window.set_title(&page.title);
            }
        } else if closed != 0 {
            self.window.close();
        } else {
            self.message(Some("No tabs. Ctrl+Shift+T opens a terminal."));
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
        let mut config = self.config.clone();
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
                self.pending_tabs
                    .set(self.pending_tabs.get().saturating_add(1));
                self.send(0, WIRE_NEW, bytes);
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
    App::open(config)?;
    let _broker = broker::serve(listener, candidate)?;
    gtk::main();
    Ok(())
}
