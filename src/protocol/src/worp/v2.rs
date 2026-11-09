//! Binding between Worp operations and the existing version-2 workspace envelope.
use super::{Reply, Request};
use crate::*;
use std::io;

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_owned())
}

pub fn encode_request(request: &Request) -> io::Result<Packet> {
    request.validate()?;
    let mut body = Buffer::default();
    body.put_u32(match request {
        Request::List => WORP_LIST,
        Request::Read { .. } => WORP_READ,
        Request::Input { .. } => WORP_INPUT,
    })?;
    if let Request::Input { bytes, .. } = request {
        body.put_bytes(bytes)?;
    }
    Ok(Packet {
        kind: WIRE_WORP,
        tab: request.tab(),
        body: body.into_bytes(),
    })
}

pub fn decode_request(packet: &Packet) -> io::Result<Request> {
    if packet.kind != WIRE_WORP {
        return Err(invalid("Expected a Worp request."));
    }
    let mut payload = Payload::new(&packet.body);
    let operation = payload.u32()?;
    let bytes = payload.take(packet.body.len() - 4)?;
    let request = match operation {
        WORP_LIST if packet.tab == 0 && bytes.is_empty() => Request::List,
        WORP_READ if bytes.is_empty() => Request::Read { tab: packet.tab },
        WORP_INPUT => Request::Input {
            tab: packet.tab,
            bytes: bytes.to_vec(),
        },
        _ => return Err(invalid("Invalid Worp operation; run worminald api help.")),
    };
    request.validate()?;
    Ok(request)
}

pub fn encode_reply(tab: u32, reply: &Reply) -> io::Result<Packet> {
    let mut body = Buffer::default();
    body.put_u32(u32::from(reply.success))?;
    body.put_bytes(reply.json.as_bytes())?;
    Ok(Packet {
        kind: WIRE_WORP,
        tab,
        body: body.into_bytes(),
    })
}

pub fn decode_reply(tab: u32, packet: Packet) -> io::Result<Reply> {
    if packet.kind != WIRE_WORP || packet.tab != tab {
        return Err(invalid("Unexpected Worp reply."));
    }
    let mut payload = Payload::new(&packet.body);
    let success = match payload.u32()? {
        0 => false,
        1 => true,
        _ => return Err(invalid("Invalid Worp reply status.")),
    };
    let json = std::str::from_utf8(payload.take(packet.body.len() - 4)?)
        .map_err(|_| invalid("Invalid UTF-8 Worp reply."))?
        .to_owned();
    Ok(Reply { success, json })
}
