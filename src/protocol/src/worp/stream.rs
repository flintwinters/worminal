//! Worp delivery over an ordered, reliable byte stream using the existing v2 envelope.
//! Unix sockets, TCP, SSH pipes and stdio can provide the same Read/Write halves.
//! This adapter does not turn raw UDP datagrams into a reliable stream.
use super::{v2, Handler, Reply, Request, Transport};
use crate::{Packet, Payload, HEADER_BYTES, MAX_PAYLOAD, WIRE_VERSION};
use std::io::{self, Read, Write};

pub struct Stream<R, W> {
    reader: R,
    writer: W,
}

impl<R: Read, W: Write> Stream<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Self { reader, writer }
    }
}

pub(super) fn read_packet(reader: &mut impl Read) -> io::Result<Packet> {
    let mut header = [0u8; HEADER_BYTES];
    reader.read_exact(&mut header).map_err(|error| {
        io::Error::new(
            error.kind(),
            "No Worp message; this daemon may need upgrading. No automatic retry was attempted.",
        )
    })?;
    let mut header = Payload::new(&header);
    let version = header.u32()?;
    let kind = header.u32()?;
    let tab = header.u32()?;
    let size = header.u32()? as usize;
    if version != WIRE_VERSION || size > MAX_PAYLOAD {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid Worp stream envelope.",
        ));
    }
    let mut body = vec![0; size];
    reader.read_exact(&mut body)?;
    Ok(Packet { kind, tab, body })
}

impl<R: Read, W: Write> Transport for Stream<R, W> {
    fn exchange(&mut self, request: Request) -> io::Result<Reply> {
        self.writer
            .write_all(&v2::encode_request(&request)?.encode()?)?;
        self.writer.flush()?;
        v2::decode_reply(request.tab(), read_packet(&mut self.reader)?)
    }
}

/// Bind a transport-neutral handler to one stream request. The adapter authenticates
/// its peer and sets deadlines before calling this function; no retries occur here.
pub fn serve_once(
    mut reader: impl Read,
    mut writer: impl Write,
    handler: &mut impl Handler,
) -> io::Result<()> {
    let request = v2::decode_request(&read_packet(&mut reader)?)?;
    let tab = request.tab();
    let reply = handler.handle(request)?;
    writer.write_all(&v2::encode_reply(tab, &reply)?.encode()?)?;
    writer.flush()
}
