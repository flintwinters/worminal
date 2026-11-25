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
const REFRESH: u32 = 105;
const COMMITTED: u32 = 106;
const ATOMIC_STATE: &[u8] = b"atomic-state-v1";
const FINAL_STATE: &[u8] = b"final-state-v1";
const LIMIT: Duration = Duration::from_secs(gui_handoff::TIMEOUT_SECONDS);
const PREPARE_LIMIT: Duration = Duration::from_secs(180);
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
pub(super) fn windows() -> Vec<Rc<App>> {
    WINDOWS.with(|slot| {
        slot.borrow()
            .iter()
            .filter(|app| !app.closing.get())
            .cloned()
            .collect()
    })
}
fn trace(message: &str) {
    worminal_workspace::diagnostics::log(libc::LOG_INFO, message);
    eprintln!("worminal: {message}");
}
fn update_status(text: &str) {
    worminal_workspace::diagnostics::log(libc::LOG_NOTICE, text);
    for app in windows() {
        app.message(Some(text));
    }
}
fn desktop_notice(text: &str) {
    if worminal_config::get().update_notifications {
        alert(text);
    }
}
fn notice(text: &str) {
    update_status(text);
    desktop_notice(text);
}
/// Version sync failures need the user's action (e.g. a peer's release cannot run here), so they reach
/// the desktop regardless of update_notifications, which covers local window-update progress.
pub(super) fn alert(text: &str) {
    if let Some(app) = windows().first() {
        app.notification_desktop
            .show(super::notifications::Message::update(text), |_| {});
    }
}
/// Every link has nothing more to wait for. The only link condition an update may wait on.
pub(super) fn links_quiet() -> bool {
    windows().iter().all(|app| app.primary.quiet())
}
pub(super) type UpdateBarrier = (Rc<super::Link>, u64);
fn barriers_complete(barriers: &[UpdateBarrier]) -> bool {
    let active = windows()
        .iter()
        .map(|app| app.primary.clone())
        .collect::<Vec<_>>();
    barriers.iter().all(|(link, revision)| {
        // A lost link cannot acknowledge a fence until its peer returns; it must not hold the update.
        !active.iter().any(|current| Rc::ptr_eq(current, link))
            || link.down()
            || link.update_fences.borrow().complete(*revision)
    })
}
fn snapshot() -> io::Result<Vec<Window>> {
    windows()
        .into_iter()
        .map(|app| {
            let (x, y) = app.window.position();
            let (width, height) = app.window.size();
            let state = app
                .window
                .window()
                .map(|window| window.state())
                .unwrap_or_else(gtk::gdk::WindowState::empty);
            let flags = u32::from(app.window.is_maximized())
                | (u32::from(state.contains(gtk::gdk::WindowState::FULLSCREEN)) << 1)
                | (u32::from(app.window.is_active()) << 2)
                | (u32::from(app.close_after_update.get()) * gui_handoff::CLOSE_REQUESTED);
            let zoom = app.fonts.borrow().zoom();
            Ok(Window {
                id: app.state_id,
                request: app.config.replacement_request()?,
                // Masters are the daemon's; windows carry none.
                links: Vec::new(),
                selected: app.active.get(),
                geometry: [x, y, width, height],
                flags,
                zoom,
                arrangement: app.arrangement()?,
                desktop: crate::graphics_ffi::desktop(&app.window)
                    .unwrap_or(gui_handoff::NO_DESKTOP),
            })
        })
        .collect()
}
fn freeze(value: bool) {
    for app in windows() {
        app.replacing.set(value);
        app.handoff_frozen.set(value);
        app.window.set_sensitive(!value);
        if !value {
            app.resume_update_reads();
        }
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
        app.show_visible_browsers();
    }
    if let Some(app) = apps.iter().find(|app| app.window.is_active()) {
        app.window.present();
    }
    for app in apps {
        if app.close_after_update.replace(false) {
            app.window.close();
        }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Peer,
    Preparing,
    Draining,
    Refreshing,
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
    ready: bool,
    committed: bool,
    barriers: Vec<UpdateBarrier>,
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
impl Transaction {
    fn timeout_reason(&self) -> String {
        if matches!(self.phase, Phase::Draining) {
            for app in windows() {
                let link = &app.primary;
                let fence_pending = self.barriers.iter().any(|(pending, revision)| {
                    Rc::ptr_eq(pending, link) && !link.update_fences.borrow().complete(*revision)
                });
                let connection = link.connection.borrow();
                trace(&format!(
                    "GUI update drain: window={} connected={} catalog_pending={} \
                     pending_tabs={} queued_bytes={} fence_pending={fence_pending}",
                    app.state_id,
                    connection.is_some(),
                    link.initial_claim.get(),
                    link.pending_tabs.get(),
                    connection.as_ref().map_or(0, |connection| connection.bytes),
                ));
            }
        }
        format!("timed out during {:?}", self.phase)
    }
}
pub(super) struct Watcher {
    broker: RefCell<Rc<broker::Server>>,
    candidate: bool,
    monitors: RefCell<Vec<gio::FileMonitor>>,
    timer: RefCell<Option<glib::SourceId>>,
    due: Cell<Instant>,
    attempted: RefCell<Vec<Option<Identity>>>,
    deferred: Cell<bool>,
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
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "A window launch is pending",
            ));
        }
        let state = snapshot()?;
        gui_handoff::encode(&state)?;
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
            ready: false,
            committed: false,
            barriers: Vec::new(),
            target: target(),
            lease,
            cookies: Vec::new(),
            result: Rc::new(RefCell::new(None)),
            transfer: None,
            channel: None,
            phase: Phase::Peer,
            deadline: Instant::now() + PREPARE_LIMIT,
            workers: Workers(Vec::new()),
            retired: false,
            rollback_started: false,
            retirement_known: true,
        });
        Ok(())
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
            if !self.broker.borrow().idle() {
                self.due.set(Instant::now() + Duration::from_millis(50));
                return true;
            }
            if let Err(error) = self.begin() {
                // Connection/launch readiness can change without another installation
                // event. Do not consume this build's attempt until staging can begin.
                if error.kind() == io::ErrorKind::WouldBlock {
                    if !self.deferred.replace(true) {
                        notice(&format!("Update pending: {error}."));
                    }
                    self.due.set(Instant::now() + Duration::from_secs(1));
                    return true;
                }
                *self.attempted.borrow_mut() = current;
                notice(&format!("Update pending: {error}."));
                return false;
            }
            self.deferred.set(false);
            *self.attempted.borrow_mut() = current;
            // Desktop shells can remove a notification when its sender exits at cutover.
            // The replacement sends the desktop notice once it owns the windows.
            update_status("Update started: replacement owner is being prepared.");
        }
        let mut slot = self.transaction.borrow_mut();
        let transaction = slot.as_mut().unwrap();
        if !matches!(transaction.phase, Phase::RollingBack) {
            if Instant::now() > transaction.deadline {
                let reason = transaction.timeout_reason();
                self.rollback(transaction, &reason);
            } else if transaction.channel.is_none()
                && transaction.child.as_ref().is_some_and(spawn::Child::exited)
            {
                self.rollback(transaction, "replacement exited before connecting");
            }
        }
        let previous = transaction.phase;
        let result = self.advance(transaction);
        if previous != transaction.phase {
            trace(&format!(
                "GUI update: {previous:?} -> {:?}",
                transaction.phase
            ));
        }
        match result {
            Ok(true) => {
                slot.take();
                // An installation can advance while a candidate is being retired. The latest
                // target still needs its own attempt even if its file event arrived mid-transaction.
                if target() != *self.attempted.borrow() {
                    self.due.set(Instant::now() + Duration::from_millis(50));
                    true
                } else {
                    false
                }
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
            notice(if transaction.retired {
                "Update failed; previous windows and terminal sessions remain. Browser pages were reloaded."
            } else {
                "Update could not prepare a replacement; current windows and terminal sessions remain."
            });
            return Ok(true);
        }
        if matches!(transaction.phase, Phase::Peer) {
            if let Some(receiver) = &transaction.transfer {
                match receiver.try_recv() {
                    Ok(result) => {
                        transaction.channel = Some(channel(result?)?);
                        transaction.transfer = None;
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
        if matches!(transaction.phase, Phase::Preparing)
            && transaction.ready
            && self.broker.borrow().idle()
            && links_quiet()
        {
            if target() != transaction.target {
                return Err(io::Error::other("Installation changed during preparation"));
            }
            self.broker.borrow().pause();
            freeze(true);
            transaction.barriers = windows()
                .iter()
                .flat_map(|app| app.update_barriers())
                .collect();
            transaction.deadline = Instant::now() + LIMIT;
            transaction.phase = Phase::Draining;
        }
        if matches!(transaction.phase, Phase::Draining)
            && barriers_complete(&transaction.barriers)
            && links_quiet()
        {
            for app in windows() {
                app.pause_update_reads();
            }
            // This is the authoritative topology boundary. The warm-up snapshot is disposable;
            // input is drained and broker/window edits are now gated, so topology is stable.
            transaction.state = Some(snapshot()?);
            transaction.lease = browser::lease()?;
            send(transaction.channel.as_mut().unwrap(), REFRESH, Vec::new())?;
            transaction.phase = Phase::Refreshing;
        }
        if matches!(transaction.phase, Phase::Refreshing) {
            if let Some(receiver) = &transaction.transfer {
                match receiver.try_recv() {
                    Ok(result) => {
                        result?;
                        transaction.transfer = None;
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                    Err(_) => return Err(io::Error::other("Window refresh transfer failed")),
                }
            } else if transaction.state.is_some() {
                if let Ok((mut peer, _)) = transaction.listener.accept() {
                    if socket::peer_pid(peer.as_raw_fd())?
                        != transaction.child.as_ref().unwrap().pid()
                    {
                        return Err(io::Error::other("Unexpected window refresh peer"));
                    }
                    let listener = self.broker.borrow().listener()?;
                    let lease = transaction
                        .lease
                        .as_ref()
                        .map(File::try_clone)
                        .transpose()?
                        .unwrap_or(File::open("/dev/null")?);
                    let state = transaction.state.take().unwrap();
                    let (sender, receiver) = mpsc::sync_channel(1);
                    transaction.transfer = Some(receiver);
                    std::thread::spawn(move || {
                        let result = (|| {
                            peer.set_write_timeout(Some(LIMIT))?;
                            gui_handoff::send(&mut peer, &listener, &lease, &state)?;
                            Ok(peer)
                        })();
                        let _ = sender.send(result);
                    });
                }
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
                if packet.kind == COMMITTED && matches!(transaction.phase, Phase::Finalizing) {
                    if !packet.body.is_empty() {
                        return Err(io::Error::other("Invalid GUI commit receipt"));
                    }
                    transaction.committed = true;
                } else if packet.kind == READY && matches!(transaction.phase, Phase::Preparing) {
                    if packet.body != ATOMIC_STATE {
                        return Err(io::Error::other(
                            "Candidate does not support atomic window refresh",
                        ));
                    }
                    transaction.ready = true;
                } else if packet.kind == READY && matches!(transaction.phase, Phase::Refreshing) {
                    if packet.body != FINAL_STATE || transaction.state.is_some() {
                        return Err(io::Error::other("Invalid refreshed window acknowledgement"));
                    }
                    transaction.transfer.take();
                    discard_views();
                    let result = transaction.result.clone();
                    if transaction.lease.is_some() {
                        browser::cookies(None, move |value| *result.borrow_mut() = Some(value))?;
                    } else {
                        *result.borrow_mut() = Some(Ok(Vec::new()));
                    }
                    transaction.phase = Phase::Exporting;
                } else if packet.kind == LIVE && matches!(transaction.phase, Phase::Activating) {
                    if target() != transaction.target {
                        return Err(io::Error::other("installation changed during replacement"));
                    }
                    let channel = transaction.channel.as_mut().unwrap();
                    let views = snapshot()?
                        .iter()
                        .map(Window::presentation)
                        .collect::<Vec<_>>();
                    send(channel, FINISH, gui_handoff::encode_presentations(&views)?)?;
                    channel.flush()?;
                    transaction.phase = Phase::Finalizing;
                }
            }
        }
        if matches!(transaction.phase, Phase::Finalizing)
            && transaction.committed
            && transaction
                .channel
                .as_ref()
                .is_some_and(|channel| channel.output.is_empty())
        {
            trace("GUI update committed");
            for app in windows() {
                app.handoff_frozen.set(false);
                app.replacing.set(false);
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
        deferred: Cell::new(false),
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

#[path = "app_upgrade_receive.rs"]
mod receiver;
pub(super) use receiver::receive;
#[path = "app_upgrade_windows.rs"]
mod candidate_windows;

#[cfg(test)]
mod contract_tests {
    use super::*;

    /// The donor of the next update is whatever binary is running now, so every message kind and
    /// capability a release has shipped must keep its meaning; new behavior takes a new number or
    /// capability string. Changing these strands running owners until they are restarted.
    #[test]
    fn shipped_wire_vocabulary_is_frozen() {
        let kinds = [READY, ACTIVATE, LIVE, FINISH, ABORT, REFRESH, COMMITTED];
        assert_eq!(kinds, [100, 101, 102, 103, 104, 105, 106]);
        assert_eq!(ATOMIC_STATE, b"atomic-state-v1");
        assert_eq!(FINAL_STATE, b"final-state-v1");
    }
}
