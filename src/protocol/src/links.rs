//! OSC 8 link runs of one row. Link ids are private to the engine that owns them, so a run carries its
//! URI inline. The encoder and the validating decoder live here so both ends share every rule.
use crate::{invalid, Buffer, Payload};
use std::io;

pub const LINK_URI_MAX: usize = 2048;
/// Encoded bytes one row may carry; later runs are dropped rather than growing a frame without bound.
pub const LINK_ROW_MAX: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkRun {
    pub column: u32,
    pub count: u32,
    pub uri: String,
}

/// Runs of adjacent equal ids with a usable URI. Ids of unusable URIs (non-UTF-8, empty, over
/// `LINK_URI_MAX`) and runs beyond `LINK_ROW_MAX` are omitted: the text stays visible, only unclickable.
pub fn link_runs<'a>(ids: &[u16], uri: impl Fn(u16) -> Option<&'a [u8]>) -> Vec<LinkRun> {
    let (mut runs, mut bytes) = (Vec::<LinkRun>::new(), 0);
    let mut column = 0;
    while column < ids.len() {
        let id = ids[column];
        let count = ids[column..].iter().take_while(|next| **next == id).count();
        let usable = (id != 0)
            .then(|| uri(id))
            .flatten()
            .filter(|bytes| (1..=LINK_URI_MAX).contains(&bytes.len()))
            .and_then(|bytes| std::str::from_utf8(bytes).ok());
        if let Some(uri) = usable {
            bytes += 12 + uri.len();
            if bytes > LINK_ROW_MAX {
                break;
            }
            runs.push(LinkRun {
                column: column as u32,
                count: count as u32,
                uri: uri.to_owned(),
            });
        }
        column += count;
    }
    runs
}

/// Encoded run list (without the row number); empty input clears a row's links.
pub fn encode_link_runs(runs: &[LinkRun]) -> io::Result<Vec<u8>> {
    let mut buffer = Buffer::default();
    for run in runs {
        buffer.put_u32(run.column)?;
        buffer.put_u32(run.count)?;
        buffer.put_string(run.uri.as_bytes())?;
    }
    Ok(buffer.into_bytes())
}

/// Consumes the rest of `body`: ordered, non-overlapping, in-bounds runs with 1..=LINK_URI_MAX UTF-8 bytes.
pub fn decode_link_runs(mut body: Payload, columns: u32) -> io::Result<Vec<LinkRun>> {
    let (mut runs, mut next) = (Vec::new(), 0u32);
    while !body.is_empty() {
        let column = body.u32()?;
        let count = body.u32()?;
        let uri = body.string()?;
        if column < next
            || count == 0
            || column > columns
            || count > columns - column
            || !(1..=LINK_URI_MAX).contains(&uri.len())
        {
            return Err(invalid("invalid link run"));
        }
        let uri = std::str::from_utf8(uri).map_err(|_| invalid("link URI is not UTF-8"))?;
        next = column + count;
        runs.push(LinkRun {
            column,
            count,
            uri: uri.to_owned(),
        });
    }
    Ok(runs)
}
