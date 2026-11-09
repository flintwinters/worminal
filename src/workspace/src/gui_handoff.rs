//! Private same-user GUI replacement envelope. Terminal ownership stays in worminald.
use crate::{broker, rights, socket};
use std::{
    fs::File,
    io::{self, Read, Write},
    os::{
        fd::{AsFd, AsRawFd},
        unix::net::{UnixListener, UnixStream},
    },
};
use worminal_protocol::{Buffer, Decoder, Payload};

pub const MAX_WINDOWS: usize = 32;
pub const TIMEOUT_SECONDS: u64 = 15;
const VERSION: u32 = 1;

pub struct Window {
    pub request: broker::Request,
    pub selected: u32,
    pub geometry: [i32; 4],
    pub flags: u32,
    pub zoom: f64,
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid GUI replacement state")
}

pub fn encode(windows: &[Window]) -> io::Result<Vec<u8>> {
    if windows.is_empty() || windows.len() > MAX_WINDOWS {
        return Err(invalid());
    }
    let mut body = Buffer::default();
    body.put_u32(VERSION)?;
    body.put_u32(windows.len() as u32)?;
    for window in windows {
        if window.geometry[2] <= 0
            || window.geometry[3] <= 0
            || window.flags > 7
            || !window.zoom.is_finite()
            || !(-10.0..=40.0).contains(&window.zoom)
        {
            return Err(invalid());
        }
        body.put_u32(window.selected)?;
        for value in window.geometry {
            body.put_u32(value as u32)?;
        }
        body.put_u32(window.flags)?;
        body.put_u32((window.zoom.to_bits() >> 32) as u32)?;
        body.put_u32(window.zoom.to_bits() as u32)?;
        body.put_string(&window.request.encode()?)?;
    }
    Ok(body.into_bytes())
}

pub fn send(
    peer: &mut UnixStream,
    listener: &UnixListener,
    lease: &File,
    windows: &[Window],
) -> io::Result<()> {
    socket::peer_pid(peer.as_raw_fd())?;
    let body = encode(windows)?;
    let mut descriptors = vec![listener.as_fd(), lease.as_fd()];
    descriptors.extend(windows.iter().map(|window| window.request.output.as_fd()));
    rights::send_descriptors(peer.as_fd(), &descriptors, b'G')?;
    peer.write_all(&(body.len() as u32).to_be_bytes())?;
    peer.write_all(&body)
}

pub fn receive(peer: &mut UnixStream) -> io::Result<(UnixListener, File, Vec<Window>)> {
    socket::peer_pid(peer.as_raw_fd())?;
    let mut descriptors =
        rights::receive_descriptors(peer.as_fd(), b'G', MAX_WINDOWS + 2)?.into_iter();
    let listener = UnixListener::from(descriptors.next().ok_or_else(invalid)?);
    let lease = File::from(descriptors.next().ok_or_else(invalid)?);
    let mut length = [0; 4];
    peer.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length > worminal_protocol::MAX_PAYLOAD {
        return Err(invalid());
    }
    let mut bytes = vec![0; length];
    peer.read_exact(&mut bytes)?;
    let mut body = Payload::new(&bytes);
    if body.u32()? != VERSION {
        return Err(invalid());
    }
    let count = body.u32()? as usize;
    if count == 0 || count > MAX_WINDOWS || descriptors.len() != count {
        return Err(invalid());
    }
    let mut windows = Vec::new();
    for output in descriptors {
        let selected = body.u32()?;
        let geometry = [
            body.u32()? as i32,
            body.u32()? as i32,
            body.u32()? as i32,
            body.u32()? as i32,
        ];
        let flags = body.u32()?;
        let zoom = f64::from_bits((u64::from(body.u32()?) << 32) | u64::from(body.u32()?));
        let bytes = body.string()?;
        let (used, packet) = Decoder::default().push(bytes)?;
        if used != bytes.len() {
            return Err(invalid());
        }
        let request = broker::Request::decode(packet.ok_or_else(invalid)?, File::from(output))?;
        windows.push(Window {
            request,
            selected,
            geometry,
            flags,
            zoom,
        });
    }
    body.finish()?;
    // Apply identical bounds to received and locally captured state.
    encode(&windows)?;
    Ok((listener, lease, windows))
}
