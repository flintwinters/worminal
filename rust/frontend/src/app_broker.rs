//! One candidate GTK owner per display/workspace. Pending launches are bounded and never block GTK.
use super::{App, Config};
use glib::{source::unix_fd_add_local, ControlFlow, IOCondition, SourceId};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    io::{self, Write},
    os::{
        fd::AsRawFd,
        linux::net::SocketAddrExt,
        unix::{
            ffi::OsStrExt,
            net::{SocketAddr, UnixListener, UnixStream},
        },
    },
    rc::Rc,
    time::Duration,
};
use worminal_workspace::broker::{self, Forward, Incoming, Request};

pub(super) fn forward(request: &Request) -> io::Result<Option<UnixListener>> {
    let mut checksum = glib::Checksum::new(glib::ChecksumType::Sha256).unwrap();
    for name in [
        "GDK_BACKEND",
        "WAYLAND_DISPLAY",
        "DISPLAY",
        "WORMINAL_SHARED_SOCKET_SCOPE",
    ] {
        let value = std::env::var_os(name).unwrap_or_default();
        checksum.update(&(value.as_bytes().len() as u64).to_be_bytes());
        checksum.update(value.as_bytes());
    }
    let hash = checksum.string().unwrap();
    let address = SocketAddr::from_abstract_name(format!(
        "worminal-rust-ui-{}-{}",
        super::uid()?,
        &hash[..48]
    ))?;
    match broker::forward(&address, request)? {
        Forward::Owner(listener) => Ok(Some(listener)),
        Forward::Complete => Ok(None),
    }
}
struct Peer {
    socket: RefCell<UnixStream>,
    input: RefCell<Incoming>,
    reader: RefCell<Option<SourceId>>,
    timeout: RefCell<Option<SourceId>>,
}
pub(super) struct Server {
    listener: UnixListener,
    reader: RefCell<Option<SourceId>>,
    peers: RefCell<BTreeMap<u64, Rc<Peer>>>,
    next: Cell<u64>,
}
impl Drop for Peer {
    fn drop(&mut self) {
        if let Some(source) = self.reader.get_mut().take() {
            source.remove();
        }
        if let Some(source) = self.timeout.get_mut().take() {
            source.remove();
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        if let Some(source) = self.reader.get_mut().take() {
            source.remove();
        }
    }
}
pub(super) fn serve(listener: UnixListener) -> io::Result<Rc<Server>> {
    listener.set_nonblocking(true)?;
    let server = Rc::new(Server {
        listener,
        reader: RefCell::new(None),
        peers: RefCell::new(BTreeMap::new()),
        next: Cell::new(0),
    });
    *server.reader.borrow_mut() = Some(unix_fd_add_local(
        server.listener.as_raw_fd(),
        IOCondition::IN,
        glib::clone!(@weak server => @default-return ControlFlow::Break, move |_, _| {
            for _ in 0..8 {
                let Ok((socket, _)) = server.listener.accept() else { break; };
                if server.peers.borrow().len() >= 32 { continue; }
                if socket.set_nonblocking(true).is_err() { continue; }
                let id = server.next.get(); server.next.set(id.wrapping_add(1));
                let peer = Rc::new(Peer {socket: RefCell::new(socket), input: RefCell::new(Incoming::default()),
                    reader: RefCell::new(None), timeout: RefCell::new(None)});
                *peer.reader.borrow_mut() = Some(unix_fd_add_local(peer.socket.borrow().as_raw_fd(),
                    IOCondition::IN | IOCondition::HUP | IOCondition::ERR,
                    glib::clone!(@weak server, @weak peer => @default-return ControlFlow::Break, move |_, _| {
                        let result = peer.input.borrow_mut().read(&mut peer.socket.borrow_mut());
                        match result {
                            Ok(None) => return ControlFlow::Continue,
                            Err(error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted)
                                => return ControlFlow::Continue,
                            result => {
                                let result = result.and_then(|request| {
                                    match Config::parse(&request.expect("complete launch"))? {
                                        Some(config) => App::open(config).map(|_| ()), None => Ok(()),
                                    }
                                });
                                if let Err(error) = &result { eprintln!("worminal-rust: forwarded launch: {error}"); }
                                let _ = peer.socket.borrow_mut().write_all(&[u8::from(result.is_err())]);
                            }
                        }
                        peer.reader.borrow_mut().take();
                        if let Some(source) = peer.timeout.borrow_mut().take() { source.remove(); }
                        server.peers.borrow_mut().remove(&id);
                        ControlFlow::Break
                    })));
                *peer.timeout.borrow_mut() = Some(glib::timeout_add_local_once(Duration::from_secs(5),
                    glib::clone!(@weak server, @weak peer => move || {
                        peer.timeout.borrow_mut().take();
                        if let Some(source) = peer.reader.borrow_mut().take() { source.remove(); }
                        server.peers.borrow_mut().remove(&id);
                    })));
                server.peers.borrow_mut().insert(id, peer);
            }
            ControlFlow::Continue
        }),
    ));
    Ok(server)
}
