use std::{
    ffi::CStr,
    io,
    os::linux::net::SocketAddrExt,
    os::unix::net::{SocketAddr, UnixListener, UnixStream},
};
use worminal_protocol::WORKSPACE_ENDPOINT;

fn address(uid: u32, scope: &[u8]) -> io::Result<SocketAddr> {
    let endpoint =
        CStr::from_bytes_with_nul(WORKSPACE_ENDPOINT).expect("generated endpoint is a C string");
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
