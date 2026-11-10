//! Binding between Worp operations and the existing version-2 workspace envelope.
use super::{devices::Device, Reply, Request};
use crate::*;
use std::io;

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_owned())
}

fn text(fields: &mut Payload<'_>) -> io::Result<String> {
    std::str::from_utf8(fields.string()?)
        .map(str::to_owned)
        .map_err(|_| invalid("Field must be UTF-8."))
}

pub fn encode_request(request: &Request) -> io::Result<Packet> {
    request.validate()?;
    let mut body = Buffer::default();
    body.put_u32(match request {
        Request::List => WORP_LIST,
        Request::OpenTerminal { .. } => WORP_OPEN_TERMINAL,
        Request::OpenBrowser { .. } => WORP_OPEN_BROWSER,
        Request::Close { .. } => WORP_CLOSE,
        Request::Read { .. } => WORP_READ,
        Request::Input { .. } => WORP_INPUT,
        Request::Devices => WORP_DEVICES,
        Request::Enroll { .. } => WORP_ENROLL,
        Request::Forget { .. } => WORP_FORGET,
    })?;
    match request {
        Request::Input { bytes, .. } => body.put_bytes(bytes)?,
        Request::OpenTerminal { options } => {
            body.put_u32(options.columns)?;
            body.put_u32(options.rows)?;
            body.put_string(options.directory.as_bytes())?;
            body.put_string(options.title.as_bytes())?;
            body.put_u32(options.command.len() as u32)?;
            for arg in &options.command {
                body.put_string(arg.as_bytes())?;
            }
        }
        Request::OpenBrowser { url } => body.put_string(url.as_bytes())?,
        Request::Enroll { device } => {
            body.put_string(device.name.as_bytes())?;
            body.put_string(device.endpoint.as_bytes())?;
        }
        Request::Forget { name } => body.put_string(name.as_bytes())?,
        _ => {}
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
        WORP_OPEN_TERMINAL if packet.tab == 0 => {
            let mut fields = Payload::new(bytes);
            let columns = fields.u32()?;
            let rows = fields.u32()?;
            let directory = text(&mut fields)?;
            let title = text(&mut fields)?;
            let count = fields.u32()?;
            if count > WORKSPACE_MAX_ARGS {
                return Err(invalid("Too many command arguments."));
            }
            let command = (0..count)
                .map(|_| text(&mut fields))
                .collect::<io::Result<Vec<_>>>()?;
            fields.finish()?;
            Request::OpenTerminal {
                options: super::tabs::TerminalOptions {
                    columns,
                    rows,
                    directory,
                    title,
                    command,
                },
            }
        }
        WORP_OPEN_BROWSER if packet.tab == 0 => {
            let mut fields = Payload::new(bytes);
            let url = text(&mut fields)?;
            fields.finish()?;
            Request::OpenBrowser { url }
        }
        WORP_CLOSE if bytes.is_empty() => Request::Close { tab: packet.tab },
        WORP_READ if bytes.is_empty() => Request::Read { tab: packet.tab },
        WORP_INPUT => Request::Input {
            tab: packet.tab,
            bytes: bytes.to_vec(),
        },
        WORP_DEVICES if packet.tab == 0 && bytes.is_empty() => Request::Devices,
        WORP_ENROLL | WORP_FORGET if packet.tab == 0 => {
            let mut fields = Payload::new(bytes);
            let name = std::str::from_utf8(fields.string()?)
                .map_err(|_| invalid("Device name must be UTF-8."))?
                .to_owned();
            let request = if operation == WORP_ENROLL {
                let endpoint = std::str::from_utf8(fields.string()?)
                    .map_err(|_| invalid("Device endpoint must be UTF-8."))?
                    .to_owned();
                Request::Enroll {
                    device: Device { name, endpoint },
                }
            } else {
                Request::Forget { name }
            };
            fields.finish()?;
            request
        }
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
