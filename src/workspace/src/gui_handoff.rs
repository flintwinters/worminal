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
const VERSION: u32 = 3;
const MAX_DESCRIPTORS: usize = 128;
pub const MAX_LINKS: usize = 16;
/// Keep the original host available for explicit reattachment, without reconnecting on replacement.
pub const PRIMARY_DETACHED: u32 = 8;

#[path = "gui_handoff_view.rs"]
mod view;
pub use view::{decode_presentations, encode_presentations, Presentation, CLOSE_REQUESTED};

pub struct Link {
    pub index: u32,
    pub request: broker::Request,
}

pub struct Window {
    pub id: u32,
    pub request: broker::Request,
    pub links: Vec<Link>,
    pub selected: u32,
    pub geometry: [i32; 4],
    pub flags: u32,
    pub zoom: f64,
}

impl Window {
    pub fn presentation(&self) -> Presentation {
        Presentation {
            id: self.id,
            selected: self.selected,
            geometry: self.geometry,
            flags: self.flags,
            zoom: self.zoom,
        }
    }
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid GUI replacement state")
}

pub fn encode(windows: &[Window]) -> io::Result<Vec<u8>> {
    encode_with(windows, false)
}

/// Only owner/link identity participates in reuse; presentation changes are never a reason to reject an update.
pub fn configuration(windows: &[Window]) -> io::Result<Vec<u8>> {
    encode_with(windows, true)
}

fn encode_with(windows: &[Window], configuration: bool) -> io::Result<Vec<u8>> {
    if windows.is_empty() || windows.len() > MAX_WINDOWS {
        return Err(invalid());
    }
    if windows
        .iter()
        .map(|window| window.links.len() + 1)
        .sum::<usize>()
        + 2
        > MAX_DESCRIPTORS
    {
        return Err(invalid());
    }
    let mut body = Buffer::default();
    body.put_u32(VERSION)?;
    body.put_u32(windows.len() as u32)?;
    let mut ids = std::collections::BTreeSet::new();
    for window in windows {
        if !ids.insert(window.id) {
            return Err(invalid());
        }
        let presentation = window.presentation();
        // Validate the actual state even when computing a normalized configuration identity.
        if configuration {
            presentation.write(&mut Buffer::default())?;
        }
        (if configuration {
            presentation.configuration()
        } else {
            presentation
        })
        .write(&mut body)?;
        body.put_string(&window.request.encode()?)?;
        if window.links.len() > MAX_LINKS {
            return Err(invalid());
        }
        body.put_u32(window.links.len() as u32)?;
        let mut indexes = std::collections::BTreeSet::new();
        for link in &window.links {
            if !(1..=255).contains(&link.index) || !indexes.insert(link.index) {
                return Err(invalid());
            }
            body.put_u32(link.index)?;
            body.put_string(&link.request.encode()?)?;
        }
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
    for window in windows {
        descriptors.push(window.request.output.as_fd());
        descriptors.extend(window.links.iter().map(|link| link.request.output.as_fd()));
    }
    rights::send_descriptors(peer.as_fd(), &descriptors, b'G')?;
    peer.write_all(&(body.len() as u32).to_be_bytes())?;
    peer.write_all(&body)
}

pub fn receive(peer: &mut UnixStream) -> io::Result<(UnixListener, File, Vec<Window>)> {
    socket::peer_pid(peer.as_raw_fd())?;
    let mut descriptors =
        rights::receive_descriptors(peer.as_fd(), b'G', MAX_DESCRIPTORS)?.into_iter();
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
    let version = body.u32()?;
    if !(1..=VERSION).contains(&version) {
        return Err(invalid());
    }
    let count = body.u32()? as usize;
    if count == 0 || count > MAX_WINDOWS {
        return Err(invalid());
    }
    let mut windows = Vec::new();
    for index in 0..count {
        let output = descriptors.next().ok_or_else(invalid)?;
        let presentation = Presentation::read(
            &mut body,
            if version < 3 {
                Some(index as u32 + 1)
            } else {
                None
            },
        )?;
        let bytes = body.string()?;
        let (used, packet) = Decoder::default().push(bytes)?;
        if used != bytes.len() {
            return Err(invalid());
        }
        let request = broker::Request::decode(packet.ok_or_else(invalid)?, File::from(output))?;
        let mut links = Vec::new();
        if version >= 2 {
            let count = body.u32()? as usize;
            if count > MAX_LINKS {
                return Err(invalid());
            }
            for _ in 0..count {
                let index = body.u32()?;
                let bytes = body.string()?;
                let (used, packet) = Decoder::default().push(bytes)?;
                if used != bytes.len() {
                    return Err(invalid());
                }
                let output = File::from(descriptors.next().ok_or_else(invalid)?);
                links.push(Link {
                    index,
                    request: broker::Request::decode(packet.ok_or_else(invalid)?, output)?,
                });
            }
        }
        windows.push(Window {
            request,
            links,
            id: presentation.id,
            selected: presentation.selected,
            geometry: presentation.geometry,
            flags: presentation.flags,
            zoom: presentation.zoom,
        });
    }
    if descriptors.next().is_some() {
        return Err(invalid());
    }
    body.finish()?;
    // Apply identical bounds to received and locally captured state.
    encode(&windows)?;
    Ok((listener, lease, windows))
}
