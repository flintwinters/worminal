//! Safe v2 framing. wire.h owns the constants; no native-layout structures are sent on the wire.
use std::{io, mem};

#[allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]
mod constants {
    include!(concat!(env!("OUT_DIR"), "/constants.rs"));
}
pub use constants::*;

pub const MAX_PAYLOAD: usize = WIRE_MAX_BYTES as usize;
const HEADER_BYTES: usize = 4 * mem::size_of::<u32>();

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// A bounded encoder. Failed writes leave the existing encoded message unchanged.
#[derive(Debug)]
pub struct Buffer {
    bytes: Vec<u8>,
    limit: usize,
}

impl Default for Buffer {
    fn default() -> Self {
        Self::with_limit(MAX_PAYLOAD)
    }
}

impl Buffer {
    pub fn with_limit(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
        }
    }

    fn reserve(&mut self, extra: usize) -> io::Result<()> {
        if extra > self.limit.saturating_sub(self.bytes.len()) {
            return Err(invalid("wire buffer limit exceeded"));
        }
        self.bytes.reserve(extra);
        Ok(())
    }

    pub fn put_u32(&mut self, value: u32) -> io::Result<()> {
        self.put_bytes(&value.to_be_bytes())
    }

    pub fn put_bytes(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.reserve(bytes.len())?;
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    pub fn put_string(&mut self, bytes: &[u8]) -> io::Result<()> {
        let length = u32::try_from(bytes.len()).map_err(|_| invalid("wire string too large"))?;
        self.reserve(4 + bytes.len())?;
        self.bytes.extend_from_slice(&length.to_be_bytes());
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    pub fn append_packet(&mut self, packet: &Packet) -> io::Result<()> {
        self.append_packet_bytes(packet.kind, packet.tab, &packet.body)
    }

    pub fn append_packet_bytes(&mut self, kind: u32, tab: u32, body: &[u8]) -> io::Result<()> {
        if body.len() > MAX_PAYLOAD {
            return Err(invalid("wire packet too large"));
        }
        // Reserve the complete frame before publishing any header bytes to a transport queue.
        self.reserve(HEADER_BYTES + body.len())?;
        for value in [WIRE_VERSION, kind, tab, body.len() as u32] {
            self.bytes.extend_from_slice(&value.to_be_bytes());
        }
        self.bytes.extend_from_slice(body);
        Ok(())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Packet {
    pub kind: u32,
    pub tab: u32,
    pub body: Vec<u8>,
}

impl Packet {
    pub fn encode(&self) -> io::Result<Vec<u8>> {
        let mut output = Buffer::with_limit(MAX_PAYLOAD + HEADER_BYTES);
        output.append_packet(self)?;
        Ok(output.into_bytes())
    }
}

/// Borrowed payload reader. Strings remain bytes so Unix paths and environments lose no information.
pub struct Payload<'a> {
    bytes: &'a [u8],
}

impl<'a> Payload<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    pub fn take(&mut self, length: usize) -> io::Result<&'a [u8]> {
        if length > self.bytes.len() {
            return Err(invalid("truncated wire payload"));
        }
        let (result, remaining) = self.bytes.split_at(length);
        self.bytes = remaining;
        Ok(result)
    }

    pub fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn string(&mut self) -> io::Result<&'a [u8]> {
        let length = self.u32()? as usize;
        self.take(length)
    }

    pub fn finish(self) -> io::Result<()> {
        if self.bytes.is_empty() {
            Ok(())
        } else {
            Err(invalid("trailing wire payload"))
        }
    }
}

enum State {
    Header {
        bytes: [u8; HEADER_BYTES],
        used: usize,
    },
    Body {
        kind: u32,
        tab: u32,
        length: usize,
        bytes: Vec<u8>,
    },
    Failed,
}

/// Incremental framing with one pending packet and a bounded allocation after header validation.
pub struct Decoder {
    state: State,
}

impl Default for Decoder {
    fn default() -> Self {
        Self {
            state: State::Header {
                bytes: [0; HEADER_BYTES],
                used: 0,
            },
        }
    }
}

impl Decoder {
    /// Returns consumed bytes and at most one complete packet. Feed any remainder separately.
    pub fn push(&mut self, mut input: &[u8]) -> io::Result<(usize, Option<Packet>)> {
        let original = input.len();
        loop {
            match &mut self.state {
                State::Header { bytes, used } => {
                    let count = (HEADER_BYTES - *used).min(input.len());
                    bytes[*used..*used + count].copy_from_slice(&input[..count]);
                    *used += count;
                    input = &input[count..];
                    if *used != HEADER_BYTES {
                        return Ok((original - input.len(), None));
                    }
                    let mut header = Payload::new(bytes);
                    let version = header.u32()?;
                    let kind = header.u32()?;
                    let tab = header.u32()?;
                    let length = header.u32()? as usize;
                    if version != WIRE_VERSION || length > MAX_PAYLOAD {
                        // Invalid framing cannot be resynchronized by treating its payload as another header.
                        self.state = State::Failed;
                        return Err(invalid("incompatible wire version or oversized packet"));
                    }
                    self.state = State::Body {
                        kind,
                        tab,
                        length,
                        bytes: Vec::with_capacity(length),
                    };
                }
                State::Body { length, bytes, .. } => {
                    let count = (*length - bytes.len()).min(input.len());
                    bytes.extend_from_slice(&input[..count]);
                    input = &input[count..];
                    if bytes.len() != *length {
                        return Ok((original - input.len(), None));
                    }
                    let state = mem::replace(&mut self.state, Self::default().state);
                    let State::Body {
                        kind, tab, bytes, ..
                    } = state
                    else {
                        unreachable!()
                    };
                    return Ok((
                        original - input.len(),
                        Some(Packet {
                            kind,
                            tab,
                            body: bytes,
                        }),
                    ));
                }
                State::Failed => {
                    return Err(invalid("wire decoder is closed after a framing error"))
                }
            }
        }
    }
}
