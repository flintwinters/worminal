//! Existing version-2 bytes are one binding of the Worp event contract.
use super::{events::*, v2};
use crate::*;
use std::io;

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid Worp event.")
}
fn string(body: &mut Payload<'_>) -> io::Result<Vec<u8>> {
    Ok(body.string()?.to_vec())
}

fn encode_string(bytes: &[u8], body: &mut Buffer, kind: u32) -> io::Result<u32> {
    body.put_string(bytes)?;
    Ok(kind)
}

fn catalog(bytes: &[u8]) -> io::Result<Catalog> {
    let mut body = Payload::new(bytes);
    let count = body.u32()? as usize;
    if count > MAX_CATALOG_TABS {
        return Err(invalid());
    }
    let closed = body.u32()?;
    let fallback = body.u32()?;
    let mut tabs = Vec::with_capacity(count);
    let mut ids = std::collections::HashSet::new();
    for _ in 0..count {
        let id = body.u32()?;
        if id == 0 || !ids.insert(id) {
            return Err(invalid());
        }
        let kind = match body.u32()? {
            0 => TabKind::Terminal,
            1 => TabKind::Browser,
            _ => return Err(invalid()),
        };
        let url = string(&mut body)?;
        let mode = body.u32()?;
        let cursor = body.u32()?;
        let title = string(&mut body)?;
        let directory = string(&mut body)?;
        let count = body.u32()? as usize;
        if count > MAX_CATALOG_COLORS {
            return Err(invalid());
        }
        let colors = (0..count)
            .map(|_| string(&mut body))
            .collect::<io::Result<Vec<_>>>()?;
        tabs.push(TabRecord {
            id,
            kind,
            url,
            mode,
            cursor,
            title,
            directory,
            colors,
        });
    }
    body.finish()?;
    Ok(Catalog {
        closed,
        fallback,
        tabs,
    })
}

fn encode_catalog(catalog: &Catalog, output: &mut Buffer) -> io::Result<()> {
    if catalog.tabs.len() > MAX_CATALOG_TABS {
        return Err(invalid());
    }
    for value in [catalog.tabs.len() as u32, catalog.closed, catalog.fallback] {
        output.put_u32(value)?;
    }
    let mut ids = std::collections::HashSet::new();
    for tab in &catalog.tabs {
        if tab.id == 0 || !ids.insert(tab.id) || tab.colors.len() > MAX_CATALOG_COLORS {
            return Err(invalid());
        }
        encode_tab_record(tab, output)?;
    }
    Ok(())
}

/// Serialize a cached catalog record; queue and packet bounds remain the delivery owner's concern.
pub fn encode_tab_record(tab: &TabRecord, output: &mut Buffer) -> io::Result<()> {
    output.put_u32(tab.id)?;
    output.put_u32(u32::from(tab.kind == TabKind::Browser))?;
    output.put_string(&tab.url)?;
    output.put_u32(tab.mode)?;
    output.put_u32(tab.cursor)?;
    output.put_string(&tab.title)?;
    output.put_string(&tab.directory)?;
    output.put_u32(tab.colors.len() as u32)?;
    for color in &tab.colors {
        output.put_string(color)?;
    }
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "One exhaustive binding table keeps event validation beside each wire variant."
)]
pub fn decode(packet: Packet) -> io::Result<Option<Event>> {
    let tab = packet.tab;
    let mut body = Payload::new(&packet.body);
    let kind = match packet.kind {
        WIRE_CATALOG => EventKind::Catalog(catalog(&packet.body)?),
        WIRE_FRAME => {
            let columns = body.u32()?;
            let rows = body.u32()?;
            let cursor_x = body.u32()?;
            let cursor_y = body.u32()?;
            let scroll = body.u32()?;
            let alternate = body.u32()?;
            if alternate > 1 {
                return Err(invalid());
            }
            body.finish()?;
            let frame = FrameState {
                columns,
                rows,
                cursor_x,
                cursor_y,
                scroll,
                alternate: alternate != 0,
            };
            frame.validate()?;
            EventKind::FrameStart(frame)
        }
        WIRE_ROW | WIRE_ROW_SPAN => {
            let row = body.u32()?;
            let column = if packet.kind == WIRE_ROW_SPAN {
                Some(body.u32()?)
            } else {
                None
            };
            if row >= WORKSPACE_MAX_ROWS
                || column.is_some_and(|column| column >= WORKSPACE_MAX_COLS)
            {
                return Err(invalid());
            }
            let offset = if column.is_some() { 8 } else { 4 };
            let cells = CellBuffer::from_payload(packet.body, offset)?;
            if column.unwrap_or(0) as usize + cells.len() > WORKSPACE_MAX_COLS as usize {
                return Err(invalid());
            }
            return Ok(Some(Event {
                tab: packet.tab,
                kind: EventKind::Row { row, column, cells },
            }));
        }
        WIRE_ROW_LINKS => {
            let row = body.u32()?;
            if row >= WORKSPACE_MAX_ROWS {
                return Err(invalid());
            }
            EventKind::Links {
                row,
                runs: decode_link_runs(body, WORKSPACE_MAX_COLS)?,
            }
        }
        WIRE_COPY_ROWS => {
            let source = body.u32()?;
            let destination = body.u32()?;
            let count = body.u32()?;
            body.finish()?;
            validate_copy(source, destination, count)?;
            EventKind::CopyRows {
                source,
                destination,
                count,
            }
        }
        WIRE_FRAME_FINISH => {
            body.finish()?;
            EventKind::FrameFinish
        }
        WIRE_CLIPBOARD => {
            let value = string(&mut body)?;
            body.finish()?;
            EventKind::Clipboard(value)
        }
        WIRE_NOTIFICATION => {
            let value = string(&mut body)?;
            body.finish()?;
            EventKind::Notification(value)
        }
        WIRE_BELL => {
            body.finish()?;
            EventKind::Bell
        }
        WIRE_PRINT => {
            return Ok(Some(Event {
                tab: packet.tab,
                kind: EventKind::Print(packet.body),
            }))
        }
        WIRE_RELEASE => {
            body.finish()?;
            EventKind::Released
        }
        WIRE_ATTENTION => {
            let on = body.u32()?;
            body.finish()?;
            EventKind::Attention(on)
        }
        WIRE_ERROR => {
            let value = string(&mut body)?;
            body.finish()?;
            EventKind::Error(value)
        }
        WIRE_CREATED => {
            body.finish()?;
            EventKind::Created
        }
        WIRE_CAPABILITIES => {
            let bits = body.u32()?;
            body.finish()?;
            EventKind::Capabilities(bits)
        }
        WIRE_SERVICE_INFO => {
            let pid = body.u32()?;
            body.finish()?;
            EventKind::ServiceInfo(pid)
        }
        WIRE_COMMAND_DONE => {
            // Older handoff commands append an empty status string; browser replacement does not.
            if !packet.body.is_empty() && !string(&mut body)?.is_empty() {
                return Err(invalid());
            }
            body.finish()?;
            EventKind::CommandDone
        }
        WIRE_SELECT_ALL => {
            let generation = body.u32()?;
            let done = body.u32()?;
            if done > 1 {
                return Err(invalid());
            }
            EventKind::Selection {
                generation,
                done: done != 0,
                text: packet.body[8..].to_vec(),
            }
        }
        WIRE_WORP => EventKind::Reply(v2::decode_reply(packet.tab, packet)?),
        _ => return Ok(None),
    };
    Ok(Some(Event { tab, kind }))
}

fn validate_copy(source: u32, destination: u32, count: u32) -> io::Result<()> {
    if count == 0
        || count > WORKSPACE_MAX_ROWS
        || source > WORKSPACE_MAX_ROWS - count
        || destination > WORKSPACE_MAX_ROWS - count
    {
        return Err(invalid());
    }
    Ok(())
}

fn encode_links(row: u32, runs: &[LinkRun], body: &mut Buffer) -> io::Result<u32> {
    if row >= WORKSPACE_MAX_ROWS {
        return Err(invalid());
    }
    let bytes = encode_link_runs(runs)?;
    decode_link_runs(Payload::new(&bytes), WORKSPACE_MAX_COLS)?;
    body.put_u32(row)?;
    body.put_bytes(&bytes)?;
    Ok(WIRE_ROW_LINKS)
}

pub fn encode(event: &Event) -> io::Result<Packet> {
    if let EventKind::Reply(reply) = &event.kind {
        return v2::encode_reply(event.tab, reply);
    }
    let mut body = Buffer::default();
    let kind = match &event.kind {
        EventKind::Catalog(catalog) => {
            encode_catalog(catalog, &mut body)?;
            WIRE_CATALOG
        }
        EventKind::FrameStart(frame) => {
            frame.validate()?;
            for value in [
                frame.columns,
                frame.rows,
                frame.cursor_x,
                frame.cursor_y,
                frame.scroll,
                u32::from(frame.alternate),
            ] {
                body.put_u32(value)?;
            }
            WIRE_FRAME
        }
        EventKind::Row { row, column, cells } => {
            if *row >= WORKSPACE_MAX_ROWS
                || column.is_some_and(|column| column >= WORKSPACE_MAX_COLS)
            {
                return Err(invalid());
            }
            if column.unwrap_or(0) as usize + cells.len() > WORKSPACE_MAX_COLS as usize {
                return Err(invalid());
            }
            body.put_u32(*row)?;
            if let Some(column) = column {
                body.put_u32(*column)?;
            }
            body.put_bytes(cells.as_bytes())?;
            if column.is_some() {
                WIRE_ROW_SPAN
            } else {
                WIRE_ROW
            }
        }
        EventKind::CopyRows {
            source,
            destination,
            count,
        } => {
            validate_copy(*source, *destination, *count)?;
            for value in [source, destination, count] {
                body.put_u32(*value)?;
            }
            WIRE_COPY_ROWS
        }
        EventKind::Links { row, runs } => encode_links(*row, runs, &mut body)?,
        EventKind::FrameFinish => WIRE_FRAME_FINISH,
        EventKind::Clipboard(bytes) => encode_string(bytes, &mut body, WIRE_CLIPBOARD)?,
        EventKind::Notification(bytes) => encode_string(bytes, &mut body, WIRE_NOTIFICATION)?,
        EventKind::Bell => WIRE_BELL,
        EventKind::Attention(on) => {
            body.put_u32(*on)?;
            WIRE_ATTENTION
        }
        EventKind::Print(bytes) => {
            body.put_bytes(bytes)?;
            WIRE_PRINT
        }
        EventKind::Released => WIRE_RELEASE,
        EventKind::Error(bytes) => {
            body.put_string(bytes)?;
            WIRE_ERROR
        }
        EventKind::Created => WIRE_CREATED,
        EventKind::Capabilities(bits) => {
            body.put_u32(*bits)?;
            WIRE_CAPABILITIES
        }
        EventKind::ServiceInfo(pid) => {
            body.put_u32(*pid)?;
            WIRE_SERVICE_INFO
        }
        EventKind::CommandDone => WIRE_COMMAND_DONE,
        EventKind::Selection {
            generation,
            done,
            text,
        } => {
            body.put_u32(*generation)?;
            body.put_u32(u32::from(*done))?;
            body.put_bytes(text)?;
            WIRE_SELECT_ALL
        }
        EventKind::Reply(_) => unreachable!(),
    };
    Ok(Packet {
        kind,
        tab: event.tab,
        body: body.into_bytes(),
    })
}

/// Append directly from borrowed engine output; no per-row owned event or intermediate payload allocation.
pub fn append_frame(output: &mut Buffer, tab: u32, frame: FrameRef<'_>) -> io::Result<()> {
    match frame {
        FrameRef::Start(frame) => {
            frame.validate()?;
            let values = [
                frame.columns,
                frame.rows,
                frame.cursor_x,
                frame.cursor_y,
                frame.scroll,
                u32::from(frame.alternate),
            ]
            .map(u32::to_be_bytes);
            output.append_packet_bytes(WIRE_FRAME, tab, values.as_flattened())
        }
        FrameRef::Row { row, column, cells } => {
            if row >= WORKSPACE_MAX_ROWS
                || cells.is_empty()
                || cells.len() % WIRE_CELL_BYTES as usize != 0
                || cells.len() / WIRE_CELL_BYTES as usize > WORKSPACE_MAX_COLS as usize
                || column.is_some_and(|column| column >= WORKSPACE_MAX_COLS)
            {
                return Err(invalid());
            }
            if column.unwrap_or(0) as usize + cells.len() / WIRE_CELL_BYTES as usize
                > WORKSPACE_MAX_COLS as usize
            {
                return Err(invalid());
            }
            let row = row.to_be_bytes();
            if let Some(column) = column {
                output.append_packet_parts(
                    WIRE_ROW_SPAN,
                    tab,
                    &[&row, &column.to_be_bytes(), cells],
                )
            } else {
                output.append_packet_parts(WIRE_ROW, tab, &[&row, cells])
            }
        }
        FrameRef::CopyRows {
            source,
            destination,
            count,
        } => {
            validate_copy(source, destination, count)?;
            let values = [source, destination, count].map(u32::to_be_bytes);
            output.append_packet_bytes(WIRE_COPY_ROWS, tab, values.as_flattened())
        }
        FrameRef::Links { row, runs } => {
            if row >= WORKSPACE_MAX_ROWS {
                return Err(invalid());
            }
            output.append_packet_parts(WIRE_ROW_LINKS, tab, &[&row.to_be_bytes(), runs])
        }
        FrameRef::Finish => output.append_packet_bytes(WIRE_FRAME_FINISH, tab, &[]),
    }
}

#[cfg(test)]
#[path = "event_tests.rs"]
mod tests;
