//! Bounded decoding for extended agent requests.
use super::Request;
use crate::*;
use std::io;
fn text(fields: &mut Payload<'_>) -> io::Result<String> {
    String::from_utf8(fields.string()?.to_vec())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Expected UTF-8."))
}
pub(super) fn decode(operation: u32, tab: u32, bytes: &[u8]) -> io::Result<Request> {
    let mut fields = Payload::new(bytes);
    let request = if operation == WORP_READ_MANY && tab == 0 {
        let tail = fields.u32()?;
        let count = fields.u32()?;
        if count > 16 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "At most 16 tabs per read.",
            ));
        }
        Request::ReadMany {
            tail,
            tabs: (0..count)
                .map(|_| fields.u32())
                .collect::<io::Result<_>>()?,
        }
    } else {
        let guard = text(&mut fields)?;
        match operation {
            WORP_WAIT => Request::Wait {
                tab,
                after: guard,
                timeout_ms: fields.u32()?,
            },
            WORP_KEYS => {
                let count = fields.u32()?;
                if count > 256 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "At most 256 keys.",
                    ));
                }
                Request::Keys {
                    tab,
                    guard,
                    names: (0..count)
                        .map(|_| text(&mut fields))
                        .collect::<io::Result<_>>()?,
                }
            }
            WORP_PASTE => Request::Paste {
                tab,
                guard,
                text: text(&mut fields)?,
            },
            WORP_GUARDED_INPUT => Request::GuardedInput {
                tab,
                guard,
                bytes: fields.string()?.to_vec(),
            },
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid extended operation.",
                ))
            }
        }
    };
    fields.finish()?;
    Ok(request)
}
