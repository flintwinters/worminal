use std::{
    io,
    os::linux::net::SocketAddrExt,
    os::unix::net::{SocketAddr, UnixListener, UnixStream},
};

/// Abstract socket names have no filesystem permissions. Authenticate before reading descriptors or launch data.
pub fn peer_pid(fd: std::os::fd::RawFd) -> io::Result<libc::pid_t> {
    unsafe {
        let mut peer: libc::ucred = std::mem::zeroed();
        let mut size = std::mem::size_of_val(&peer) as libc::socklen_t;
        if libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            std::ptr::from_mut(&mut peer).cast(),
            &mut size,
        ) < 0
        {
            return Err(io::Error::last_os_error());
        }
        if size as usize != std::mem::size_of_val(&peer) || peer.uid != libc::getuid() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "local peer is not the same user",
            ));
        }
        Ok(peer.pid)
    }
}
use worminal_protocol::WORKSPACE_ENDPOINT;

fn address(uid: u32, scope: &[u8]) -> io::Result<SocketAddr> {
    let endpoint =
        WORKSPACE_ENDPOINT;
    let mut name = endpoint.to_bytes().to_vec();
    name.extend_from_slice(format!("-{uid}-").as_bytes());
    name.extend_from_slice(scope);
    // Legacy clients use snprintf in sun_path[1..], reserving both the abstract prefix and a trailing NUL.
    // Reject the extra name byte Linux accepts so every published endpoint remains reachable by those clients.
    let layout: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if name.len() > layout.sun_path.len() - 2 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "workspace scope is too long",
        ));
    }
    SocketAddr::from_abstract_name(name)
}

pub fn connect(uid: u32, scope: &[u8]) -> io::Result<UnixStream> {
    UnixStream::connect_addr(&address(uid, scope)?)
}

pub fn listen(uid: u32, scope: &[u8]) -> io::Result<UnixListener> {
    UnixListener::bind_addr(&address(uid, scope)?)
}
