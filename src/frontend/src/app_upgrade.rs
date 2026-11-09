//! Staged owner replacement. Old windows close only after the complete candidate is ready.
use super::{broker, App, Config, Connection, WINDOWS};
use crate::browser;
use gtk::{gio, glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    ffi::{CStr, OsStr},
    fs::File,
    io::{self, Read},
    os::{
        fd::{AsRawFd, BorrowedFd, OwnedFd},
        linux::net::SocketAddrExt,
        unix::{
            ffi::OsStringExt,
            fs::MetadataExt,
            net::{SocketAddr, UnixListener, UnixStream},
        },
    },
    path::PathBuf,
    rc::Rc,
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};
use worminal_protocol::{Packet, WORKSPACE_CLIENT_QUEUE_BYTES};
use worminal_workspace::{
    gui_handoff::{self, Window},
    socket, spawn,
};

const READY: u32 = 100;
const ACTIVATE: u32 = 101;
const LIVE: u32 = 102;
const FINISH: u32 = 103;
const ABORT: u32 = 104;
const LIMIT: Duration = Duration::from_secs(gui_handoff::TIMEOUT_SECONDS);
type Identity = (u64, u64, u64, i64, i64);
struct Artifact {
    path: PathBuf,
    _file: File,
    loaded: Identity,
}
thread_local! { static RUNTIME: RefCell<Vec<Artifact>> = const { RefCell::new(Vec::new()) }; }
fn identity(info: std::fs::Metadata) -> Identity {
    (
        info.dev(),
        info.ino(),
        info.len(),
        info.mtime(),
        info.mtime_nsec(),
    )
}
fn artifact(path: PathBuf, file: File) -> io::Result<Artifact> {
    Ok(Artifact {
        path,
        loaded: identity(file.metadata()?),
        _file: file,
    })
}
pub(crate) unsafe fn prepare_runtime(
    executable: i32,
    frontend: i32,
    executable_path: &CStr,
    frontend_path: &CStr,
) -> io::Result<()> {
    let paths = [(executable, executable_path), (frontend, frontend_path)];
    let artifacts = paths
        .into_iter()
        .map(|(fd, path)| {
            let file = File::from(unsafe { BorrowedFd::borrow_raw(fd) }.try_clone_to_owned()?);
            artifact(
                PathBuf::from(std::ffi::OsString::from_vec(path.to_bytes().to_vec())),
                file,
            )
        })
        .collect::<io::Result<Vec<_>>>()?;
    RUNTIME.with(|slot| *slot.borrow_mut() = artifacts);
    Ok(())
}
fn target() -> Vec<Option<Identity>> {
    RUNTIME.with(|slot| {
        slot.borrow()
            .iter()
            .map(|item| std::fs::metadata(&item.path).ok().map(identity))
            .collect()
    })
}
fn changed() -> bool {
    RUNTIME.with(|slot| {
        slot.borrow()
            .iter()
            .any(|item| std::fs::metadata(&item.path).ok().map(identity) != Some(item.loaded))
    })
}
fn windows() -> Vec<Rc<App>> {
    WINDOWS.with(|slot| {
        slot.borrow()
            .iter()
            .filter(|app| !app.closing.get())
            .cloned()
            .collect()
    })
}
fn notice(text: &str) {
    for app in windows() {
        app.message(Some(text));
    }
}
fn snapshot() -> io::Result<Vec<Window>> {
    windows()
        .into_iter()
        .map(|app| {
            if app.connection.borrow().is_none()
                || app.initial_claim.get()
                || app.pending_tabs.get() != 0
                || app
                    .connection
                    .borrow()
                    .as_ref()
                    .is_some_and(|connection| !connection.output.is_empty())
            {
                return Err(io::Error::other(
                    "Windows are reconnecting or have pending input",
                ));
            }
            let (x, y) = app.window.position();
            let (width, height) = app.window.size();
            let state = app
                .window
                .window()
                .map(|window| window.state())
                .unwrap_or_else(gtk::gdk::WindowState::empty);
            let flags = u32::from(app.window.is_maximized())
                | (u32::from(state.contains(gtk::gdk::WindowState::FULLSCREEN)) << 1)
                | (u32::from(app.window.is_active()) << 2);
            let zoom = app.fonts.borrow().zoom();
            Ok(Window {
                request: app.config.replacement_request()?,
                selected: app.active.get(),
                geometry: [x, y, width, height],
                flags,
                zoom,
            })
        })
        .collect()
}
fn stable_state() -> io::Result<Vec<u8>> {
    let mut state = snapshot()?;
    for window in &mut state {
        window.flags &= 3;
    }
    gui_handoff::encode(&state)
}
fn freeze(value: bool) {
    for app in windows() {
        app.replacing.set(value);
        app.window.set_sensitive(!value);
    }
}
fn discard_views() {
    for app in windows() {
        let pages = app.pages.borrow().clone();
        for page in pages {
            page.view.borrow_mut().take();
        }
    }
}
fn restore_views() {
    freeze(false);
    let apps = windows();
    for app in &apps {
        if app.active.get() != 0 {
            app.activate(app.active.get());
        }
    }
    if let Some(app) = apps.iter().find(|app| app.window.is_active()) {
        app.window.present();
    }
}
fn address(token: &str) -> io::Result<SocketAddr> {
    if token.len() != 48 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(io::Error::other("Invalid GUI replacement token"));
    }
    SocketAddr::from_abstract_name(
        format!("worminal-ui-replace-{}-{token}", super::uid()?).as_bytes(),
    )
}
fn token() -> io::Result<String> {
    let mut bytes = [0; 24];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
fn channel(socket: UnixStream) -> io::Result<Connection> {
    socket.set_nonblocking(true)?;
    Ok(Connection {
        socket,
        _ssh: None,
        decoder: Default::default(),
        output: Default::default(),
        offset: 0,
        bytes: 0,
    })
}
fn send(channel: &mut Connection, kind: u32, body: Vec<u8>) -> io::Result<()> {
    channel.queue(Packet { kind, tab: 0, body })
}

/// Pin browser descendants before destroying their manager; never overlap profile writers.
struct Workers(Vec<OwnedFd>);
impl Workers {
    fn capture(owner: libc::pid_t) -> io::Result<Self> {
        let mut processes = std::collections::HashMap::new();
        for path in std::fs::read_dir("/proc")?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
        {
            let Some(pid) = path
                .file_name()
                .and_then(OsStr::to_str)
                .and_then(|value| value.parse::<i32>().ok())
            else {
                continue;
            };
            let Ok(status) = std::fs::read_to_string(path.join("status")) else {
                continue;
            };
            let parent = status.lines().find_map(|line| {
                line.strip_prefix("PPid:")
                    .and_then(|value| value.trim().parse::<i32>().ok())
            });
            let browser = status
                .lines()
                .any(|line| line.starts_with("Name:") && line.contains("WebKit"));
            if let Some(parent) = parent {
                processes.insert(pid, (parent, browser));
            }
        }
        let mut workers = Vec::new();
        for (&pid, &(mut parent, browser)) in &processes {
            if !browser {
                continue;
            }
            for _ in 0..64 {
                if parent == owner {
                    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
                    if fd >= 0 {
                        workers.push(unsafe { std::os::fd::FromRawFd::from_raw_fd(fd as i32) });
                    } else if io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
                        return Err(io::Error::other("Cannot verify browser process retirement"));
                    }
                    break;
                }
                let Some(&(next, _)) = processes.get(&parent) else {
                    break;
                };
                if next == parent || next == 0 {
                    break;
                }
                parent = next;
            }
        }
        Ok(Self(workers))
    }
    fn done(&self) -> bool {
        self.0.iter().all(|fd| {
            let mut event = libc::pollfd {
                fd: fd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            (unsafe { libc::poll(&mut event, 1, 0) }) > 0
        })
    }
}

enum Phase {
    Peer,
    Preparing,
    Exporting,
    Retiring,
    Activating,
    Finalizing,
    RollingBack,
}
struct Transaction {
    listener: UnixListener,
    child: Option<spawn::Child>,
    state: Option<Vec<Window>>,
    encoded: Vec<u8>,
    target: Vec<Option<Identity>>,
    lease: Option<File>,
    cookies: Vec<u8>,
    result: Rc<RefCell<Option<io::Result<Vec<u8>>>>>,
    transfer: Option<Receiver<io::Result<UnixStream>>>,
    channel: Option<Connection>,
    phase: Phase,
    deadline: Instant,
    workers: Workers,
    retired: bool,
    rollback_started: bool,
    retirement_known: bool,
}
pub(super) struct Watcher {
    broker: RefCell<Rc<broker::Server>>,
    candidate: bool,
    monitors: RefCell<Vec<gio::FileMonitor>>,
    timer: RefCell<Option<glib::SourceId>>,
    due: Cell<Instant>,
    attempted: RefCell<Vec<Option<Identity>>>,
    transaction: RefCell<Option<Transaction>>,
}
impl Drop for Watcher {
    fn drop(&mut self) {
        if let Some(timer) = self.timer.get_mut().take() {
            timer.remove();
        }
    }
}
impl Watcher {
    fn arm(self: &Rc<Self>) {
        self.due.set(Instant::now() + Duration::from_millis(600));
        if self.timer.borrow().is_some() {
            return;
        }
        let weak = Rc::downgrade(self);
        *self.timer.borrow_mut() = Some(glib::timeout_add_local(
            Duration::from_millis(50),
            move || {
                let Some(owner) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                let keep = owner.tick();
                if !keep {
                    owner.timer.borrow_mut().take();
                }
                if keep {
                    glib::ControlFlow::Continue
                } else {
                    glib::ControlFlow::Break
                }
            },
        ));
    }
    fn begin(&self) -> io::Result<()> {
        if !self.broker.borrow().idle() {
            return Err(io::Error::other("A window launch is pending"));
        }
        let state = snapshot()?;
        let encoded = stable_state()?;
        let lease = browser::lease()?;
        browser::pin_for_update();
        let token = token()?;
        let listener = UnixListener::bind_addr(&address(&token)?)?;
        listener.set_nonblocking(true)?;
        let executable = RUNTIME
            .with(|slot| slot.borrow().first().map(|item| item.path.clone()))
            .ok_or_else(|| io::Error::other("Missing runtime executable"))?;
        let child = crate::startup::replacement(&executable, &token)?;
        *self.transaction.borrow_mut() = Some(Transaction {
            listener,
            child: Some(child),
            state: Some(state),
            encoded,
            target: target(),
            lease,
            cookies: Vec::new(),
            result: Rc::new(RefCell::new(None)),
            transfer: None,
            channel: None,
            phase: Phase::Peer,
            deadline: Instant::now() + LIMIT,
            workers: Workers(Vec::new()),
            retired: false,
            rollback_started: false,
            retirement_known: true,
        });
        Ok(())
    }
    fn valid(&self, transaction: &Transaction) -> bool {
        self.broker.borrow().idle()
            && target() == transaction.target
            && stable_state().ok().as_ref() == Some(&transaction.encoded)
            && browser::lease().ok().flatten().is_some() == transaction.lease.is_some()
    }
    fn rollback(&self, transaction: &mut Transaction, message: &str) {
        if matches!(transaction.phase, Phase::RollingBack) {
            return;
        }
        notice(&format!(
            "Update postponed: {message}. Current terminal sessions are preserved."
        ));
        if let Some(child) = &transaction.child {
            match Workers::capture(child.pid()) {
                Ok(mut workers) => transaction.workers.0.append(&mut workers.0),
                Err(_) => transaction.retirement_known = false,
            }
            child.signal(libc::SIGTERM);
        }
        transaction.phase = Phase::RollingBack;
        transaction.rollback_started = false;
        transaction.result.borrow_mut().take();
    }
    fn tick(self: &Rc<Self>) -> bool {
        if self.transaction.borrow().is_none() {
            if Instant::now() < self.due.get() {
                return true;
            }
            let current = target();
            if !changed()
                || current.iter().any(Option::is_none)
                || *self.attempted.borrow() == current
            {
                return false;
            }
            *self.attempted.borrow_mut() = current;
            if let Err(error) = self.begin() {
                notice(&format!("Update pending: {error}."));
                return false;
            }
        }
        let mut slot = self.transaction.borrow_mut();
        let transaction = slot.as_mut().unwrap();
        if !matches!(transaction.phase, Phase::RollingBack)
            && (Instant::now() > transaction.deadline
                || transaction.child.as_ref().is_some_and(spawn::Child::exited))
        {
            self.rollback(transaction, "replacement did not become ready");
        }
        let result = self.advance(transaction);
        match result {
            Ok(true) => {
                slot.take();
                false
            }
            Ok(false) => true,
            Err(error) => {
                self.rollback(transaction, &error.to_string());
                true
            }
        }
    }
    #[expect(
        clippy::too_many_lines,
        reason = "one ordered GUI replacement transaction"
    )]
    fn advance(&self, transaction: &mut Transaction) -> io::Result<bool> {
        if matches!(transaction.phase, Phase::RollingBack) {
            if !transaction.retirement_known
                || transaction
                    .child
                    .as_ref()
                    .is_some_and(|child| !child.exited())
                || !transaction.workers.done()
            {
                return Ok(false);
            }
            if !transaction.rollback_started {
                transaction.rollback_started = true;
                if transaction.retired {
                    if let Some(lease) = &transaction.lease {
                        browser::adopt(lease.try_clone()?)?;
                    }
                    if !transaction.cookies.is_empty() {
                        let result = transaction.result.clone();
                        browser::cookies(Some(&transaction.cookies), move |value| {
                            *result.borrow_mut() = Some(value)
                        })?;
                        return Ok(false);
                    }
                }
            } else if transaction.retired && !transaction.cookies.is_empty() {
                let Some(result) = transaction.result.borrow_mut().take() else {
                    return Ok(false);
                };
                result?;
            }
            restore_views();
            let listener = self.broker.borrow().listener()?;
            *self.broker.borrow_mut() = broker::serve(listener, self.candidate)?;
            notice("Update failed; previous windows and terminal sessions remain. Browser pages were reloaded.");
            return Ok(true);
        }
        if matches!(transaction.phase, Phase::Peer) {
            if let Some(receiver) = &transaction.transfer {
                match receiver.try_recv() {
                    Ok(result) => {
                        transaction.channel = Some(channel(result?)?);
                        transaction.phase = Phase::Preparing;
                    }
                    Err(mpsc::TryRecvError::Empty) => return Ok(false),
                    Err(_) => return Err(io::Error::other("replacement handshake failed")),
                }
            } else if let Ok((mut peer, _)) = transaction.listener.accept() {
                if socket::peer_pid(peer.as_raw_fd())? != transaction.child.as_ref().unwrap().pid()
                {
                    return Err(io::Error::other("unexpected replacement peer"));
                }
                let listener = self.broker.borrow().listener()?;
                let lease = transaction
                    .lease
                    .as_ref()
                    .map(File::try_clone)
                    .transpose()?
                    .unwrap_or(File::open("/dev/null")?);
                let state = transaction.state.take().unwrap();
                let (send, receiver) = mpsc::sync_channel(1);
                transaction.transfer = Some(receiver);
                std::thread::spawn(move || {
                    let result = (|| {
                        peer.set_write_timeout(Some(LIMIT))?;
                        gui_handoff::send(&mut peer, &listener, &lease, &state)?;
                        Ok(peer)
                    })();
                    let _ = send.send(result);
                });
                return Ok(false);
            } else {
                return Ok(false);
            }
        }
        let packets = if let Some(channel) = transaction.channel.as_mut() {
            channel.flush()?;
            channel.read()?
        } else {
            Vec::new()
        };
        {
            for packet in packets {
                if packet.kind == ABORT {
                    return Err(io::Error::other("candidate rejected the replacement"));
                }
                if packet.kind == READY && matches!(transaction.phase, Phase::Preparing) {
                    if !self.valid(transaction) {
                        return Err(io::Error::other("window state changed during preparation"));
                    }
                    self.broker.borrow().pause();
                    freeze(true);
                    discard_views();
                    let result = transaction.result.clone();
                    if transaction.lease.is_some() {
                        browser::cookies(None, move |value| *result.borrow_mut() = Some(value))?;
                    } else {
                        *result.borrow_mut() = Some(Ok(Vec::new()));
                    }
                    transaction.phase = Phase::Exporting;
                } else if packet.kind == LIVE && matches!(transaction.phase, Phase::Activating) {
                    if target() != transaction.target || stable_state()? != transaction.encoded {
                        return Err(io::Error::other("installation changed during replacement"));
                    }
                    let channel = transaction.channel.as_mut().unwrap();
                    send(channel, FINISH, Vec::new())?;
                    channel.flush()?;
                    transaction.phase = Phase::Finalizing;
                }
            }
        }
        if matches!(transaction.phase, Phase::Finalizing)
            && transaction
                .channel
                .as_ref()
                .is_some_and(|channel| channel.output.is_empty())
        {
            for app in windows() {
                app.window.close();
            }
            std::mem::forget(transaction.child.take().unwrap());
            return Ok(true);
        }
        if matches!(transaction.phase, Phase::Exporting) {
            let value = transaction.result.borrow_mut().take();
            if let Some(value) = value {
                transaction.cookies = value?;
                if transaction.cookies.len() + 16 > WORKSPACE_CLIENT_QUEUE_BYTES as usize {
                    return Err(io::Error::other("cookie transfer exceeds update queue"));
                }
                transaction.workers = Workers::capture(std::process::id() as i32)?;
                browser::quiesce()?;
                transaction.retired = true;
                transaction.phase = Phase::Retiring;
            }
        }
        if matches!(transaction.phase, Phase::Retiring) && transaction.workers.done() {
            send(
                transaction.channel.as_mut().unwrap(),
                ACTIVATE,
                transaction.cookies.clone(),
            )?;
            transaction.phase = Phase::Activating;
        }
        Ok(false)
    }
}

pub(super) fn watch(broker: Rc<broker::Server>, candidate: bool) -> io::Result<Rc<Watcher>> {
    if RUNTIME.with(|slot| slot.borrow().is_empty()) {
        let path = std::env::current_exe()?;
        let loaded = artifact(path, File::open("/proc/self/exe")?)?;
        RUNTIME.with(|slot| slot.borrow_mut().push(loaded));
    }
    if let Some((path, file)) = browser::update_artifact()? {
        RUNTIME.with(|slot| slot.borrow_mut().push(artifact(path, file).unwrap()));
    }
    let owner = Rc::new(Watcher {
        broker: RefCell::new(broker),
        candidate,
        monitors: RefCell::new(Vec::new()),
        timer: RefCell::new(None),
        due: Cell::new(Instant::now()),
        attempted: RefCell::new(Vec::new()),
        transaction: RefCell::new(None),
    });
    let directories: std::collections::BTreeSet<_> = RUNTIME.with(|slot| {
        slot.borrow()
            .iter()
            .filter_map(|item| item.path.parent().map(PathBuf::from))
            .collect()
    });
    for directory in directories {
        let monitor = gio::File::for_path(directory)
            .monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
            .map_err(io::Error::other)?;
        let weak = Rc::downgrade(&owner);
        monitor.connect_changed(move |_, _, _, _| {
            if let Some(owner) = weak.upgrade() {
                owner.arm();
            }
        });
        owner.monitors.borrow_mut().push(monitor);
    }
    if changed() {
        owner.arm();
    }
    Ok(owner)
}

#[expect(
    clippy::too_many_lines,
    reason = "candidate handshake and GTK event loop form one ordered transaction"
)]
pub(super) fn receive(token: &OsStr, candidate: bool) -> io::Result<()> {
    let token = token
        .to_str()
        .ok_or_else(|| io::Error::other("Invalid update token"))?;
    let mut peer = UnixStream::connect_addr(&address(token)?)?;
    peer.set_read_timeout(Some(LIMIT))?;
    let (listener, lease, states) = gui_handoff::receive(&mut peer)?;
    crate::browser::configure_profile(candidate)?;
    gtk::init().map_err(io::Error::other)?;
    let _state = super::window_state::start();
    let mut focus = None;
    for state in states {
        let Some(mut config) = Config::parse(&state.request, candidate)? else {
            return Err(io::Error::other("Invalid update launch"));
        };
        config.attach = true;
        config.command.clear();
        config.browser_url = None;
        config.zoom = state.zoom;
        let app = App::open_mode(config, true)?;
        app.requested.set(state.selected);
        app.geometry_pending.set(false);
        app.window
            .set_default_size(state.geometry[2], state.geometry[3]);
        app.window.resize(state.geometry[2], state.geometry[3]);
        if crate::graphics_ffi::x11(&app.window) {
            app.window.move_(state.geometry[0], state.geometry[1]);
        }
        if state.flags & 1 != 0 {
            app.window.maximize();
        }
        if state.flags & 2 != 0 {
            app.window.fullscreen();
        }
        if state.flags & 4 != 0 {
            focus = Some(app.state_id);
        }
    }
    let connection = Rc::new(RefCell::new(channel(peer)?));
    let phase = Rc::new(Cell::new(0u32));
    let result = Rc::new(RefCell::new(None::<io::Result<Vec<u8>>>));
    let lease = Rc::new(RefCell::new(Some(lease)));
    let finished = Rc::new(Cell::new(false));
    let broker_slot = Rc::new(RefCell::new(None));
    let monitor_slot = Rc::new(RefCell::new(None));
    let listener = Rc::new(RefCell::new(Some(listener)));
    let deadline = Instant::now() + LIMIT;
    let tick = glib::timeout_add_local(Duration::from_millis(25), {
        let connection = connection.clone();
        let phase = phase.clone();
        let result = result.clone();
        let lease = lease.clone();
        let finished = finished.clone();
        let broker_slot = broker_slot.clone();
        let monitor_slot = monitor_slot.clone();
        move || {
            let action = (|| -> io::Result<bool> {
                connection.borrow_mut().flush()?;
                let packets = connection.borrow_mut().read()?;
                for packet in packets {
                    match packet.kind {
                        ACTIVATE if phase.get() == 1 => {
                            if !packet.body.is_empty() {
                                browser::adopt(lease.borrow_mut().take().unwrap())?;
                                let result = result.clone();
                                browser::cookies(Some(&packet.body), move |value| {
                                    *result.borrow_mut() = Some(value)
                                })?;
                            } else {
                                *result.borrow_mut() = Some(Ok(Vec::new()));
                            }
                            phase.set(2);
                        }
                        FINISH if phase.get() == 3 => {
                            for app in windows() {
                                app.defer_navigation.set(false);
                                let pages = app.pages.borrow().clone();
                                for page in pages {
                                    if let Some(view) = page.view.borrow().as_ref() {
                                        view.navigate(&page.url);
                                    }
                                }
                                app.window.set_sensitive(true);
                                if Some(app.state_id) == focus {
                                    app.activate(app.active.get());
                                    app.window.present();
                                }
                                if !crate::graphics_ffi::x11(&app.window) {
                                    app.message(Some("Updated; window placement is controlled by the compositor."));
                                }
                            }
                            let broker =
                                broker::serve(listener.borrow_mut().take().unwrap(), candidate)?;
                            *monitor_slot.borrow_mut() = Some(watch(broker.clone(), candidate)?);
                            *broker_slot.borrow_mut() = Some(broker);
                            finished.set(true);
                            return Ok(true);
                        }
                        ABORT => return Err(io::Error::other("replacement cancelled")),
                        _ => return Err(io::Error::other("Invalid replacement handshake")),
                    }
                }
                if phase.get() == 0
                    && windows().iter().all(|app| {
                        !app.initial_claim.get()
                            && (app.active.get() == 0
                                || app
                                    .pages
                                    .borrow()
                                    .iter()
                                    .any(|page| page.id == app.active.get()))
                    })
                {
                    if windows()
                        .iter()
                        .any(|app| app.pages.borrow().iter().any(|page| page.browser))
                    {
                        browser::preflight()?;
                    }
                    send(&mut connection.borrow_mut(), READY, Vec::new())?;
                    phase.set(1);
                }
                if phase.get() == 2 {
                    let value = result.borrow_mut().take();
                    if let Some(value) = value {
                        value?;
                        for app in windows() {
                            app.window.set_sensitive(false);
                            app.replacing.set(false);
                            crate::graphics_ffi::show_window(&app.window);
                            if app.active.get() != 0 {
                                app.activate(app.active.get());
                            }
                        }
                        phase.set(4);
                    }
                }
                if phase.get() == 4
                    && windows().iter().all(|app| {
                        app.active.get() == 0
                            || app.replacement_frame.get()
                            || app
                                .pages
                                .borrow()
                                .iter()
                                .any(|page| page.id == app.active.get() && page.browser)
                    })
                {
                    if windows().iter().any(|app| {
                        app.pages
                            .borrow()
                            .iter()
                            .any(|page| page.id == app.active.get() && page.browser)
                            && !windows().iter().any(|owner| {
                                owner.config.master == app.config.master
                                    && owner.browser(app.active.get()).is_some()
                            })
                    }) {
                        return Err(io::Error::other("Candidate browser failed to initialize"));
                    }
                    send(&mut connection.borrow_mut(), LIVE, Vec::new())?;
                    phase.set(3);
                }
                if Instant::now() > deadline {
                    return Err(io::Error::other("replacement handshake timed out"));
                }
                Ok(false)
            })();
            match action {
                Ok(false) => glib::ControlFlow::Continue,
                Ok(true) => glib::ControlFlow::Break,
                Err(_) => {
                    let _ = send(&mut connection.borrow_mut(), ABORT, Vec::new());
                    for app in windows() {
                        app.window.close();
                    }
                    glib::MainContext::default().wakeup();
                    glib::ControlFlow::Break
                }
            }
        }
    });
    gtk::main();
    if !finished.get() {
        let _ = tick;
        return Err(io::Error::other("Window replacement did not commit"));
    }
    Ok(())
}
