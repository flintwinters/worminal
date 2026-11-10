//! A viewport baseline describes queued bytes, never merely observed engine state.
use crate::{
    queue::Queue,
    state::{core, Screen},
};
use std::io;
use worminal_engine::{Cell, Engine, Frame};
use worminal_protocol::{
    encode_link_runs, link_runs, Buffer, HEADER_BYTES, WIRE_CELL_BYTES, WIRE_COPY_ROWS,
    WIRE_FEATURE_COPY_ROWS, WIRE_FEATURE_ROW_LINKS, WIRE_FEATURE_ROW_SPAN, WIRE_FRAME,
    WIRE_FRAME_FINISH, WIRE_ROW, WIRE_ROW_LINKS, WIRE_ROW_SPAN,
};

fn same_frame(a: Frame, b: Frame) -> bool {
    (
        a.columns,
        a.rows,
        a.cursor_x,
        a.cursor_y,
        a.scroll,
        a.alternate,
    ) == (
        b.columns,
        b.rows,
        b.cursor_x,
        b.cursor_y,
        b.scroll,
        b.alternate,
    )
}

#[cfg(test)]
#[path = "frame_tests.rs"]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Move {
    source: usize,
    destination: usize,
    count: usize,
}

fn row_hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(14695981039346656037, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(1099511628211)
    })
}

fn find_move(before: &[u8], current: &[Option<Vec<u8>>], width: usize) -> Option<Move> {
    let old: Vec<_> = before.chunks_exact(width).map(row_hash).collect();
    let now: Vec<_> = current
        .iter()
        .map(|row| row_hash(row.as_ref().unwrap()))
        .collect();
    let changed: Vec<_> = current
        .iter()
        .enumerate()
        .map(|(row, bytes)| before[row * width..(row + 1) * width] != **bytes.as_ref().unwrap())
        .collect();
    let mut best = None;
    let mut best_score = 1;
    let rows = current.len() as isize;
    for shift in 1 - rows..rows {
        if shift == 0 {
            continue;
        }
        let (mut run, mut score) = (0, 0);
        for row in 0..rows {
            let from = row + shift;
            if from < 0 || from >= rows || now[row as usize] != old[from as usize] {
                run = 0;
                score = 0;
                continue;
            }
            run += 1;
            score += usize::from(changed[row as usize]);
            if score > best_score
                || (score == best_score && best.is_some_and(|m: Move| run > m.count))
            {
                best_score = score;
                let destination = row as usize + 1 - run;
                best = Some(Move {
                    source: (destination as isize + shift) as usize,
                    destination,
                    count: run,
                });
            }
        }
    }
    // Hashes choose candidates; only exact bytes authorize a move in the client's replica.
    best.filter(|m| {
        (0..m.count).all(|offset| {
            let source = (m.source + offset) * width;
            before[source..source + width] == **current[m.destination + offset].as_ref().unwrap()
        })
    })
}

struct Update {
    row: usize,
    bytes: Vec<u8>,
    span: Option<std::ops::Range<usize>>,
}

struct Plan {
    frame: Frame,
    full: bool,
    movement: Option<Move>,
    updates: Vec<Update>,
    /// Rows whose encoded link runs differ from the baseline (after the move); empty = clear.
    links: Option<Vec<(usize, Vec<u8>)>>,
    bytes: Vec<u8>,
}

enum Prepared {
    Held,
    Unchanged,
    Changed(Plan),
}
#[derive(Debug, PartialEq)]
pub(crate) enum Outcome {
    Held,
    Unchanged,
    Sent,
}

impl Screen {
    #[expect(
        clippy::too_many_lines,
        reason = "frame preparation is one ordered pass over damaged rows"
    )]
    fn prepare(
        &self,
        engine: &mut Engine,
        tab: u32,
        full: bool,
        features: u32,
        reply: bool,
        now: i64,
    ) -> io::Result<Prepared> {
        let frame = core(engine.frame(now))?;
        if frame.redraw_deadline != 0 {
            return Ok(Prepared::Held);
        }
        let width = frame.columns as usize * WIRE_CELL_BYTES as usize;
        let rows = frame.rows as usize;
        let links = features & WIRE_FEATURE_ROW_LINKS != 0;
        let full = full
            || self.frame.columns != frame.columns
            || self.bytes.len() != width * rows
            || (links && self.links.len() != rows);
        let dirty: Vec<_> = (0..frame.rows)
            .map(|row| core(engine.row_dirty(row)))
            .collect::<io::Result<_>>()?;
        let dirty_count = dirty.iter().filter(|flag| **flag).count();
        let search_move = !full
            && features & WIRE_FEATURE_COPY_ROWS != 0
            && dirty_count >= 3
            && dirty_count * 2 >= rows;
        let mut current = Vec::with_capacity(rows);
        let mut cells: Vec<Cell> = Vec::new();
        let (mut ids, mut current_links) = (Vec::new(), Vec::new());
        for (row, dirty) in dirty.iter().enumerate() {
            if !full && !search_move && !dirty {
                current.push(None);
                current_links.push(None);
                continue;
            }
            core(engine.row(row as i32, &mut cells))?;
            let mut encoded = Buffer::default();
            encoded.reserve(width)?;
            for cell in &cells {
                let fields = [cell.rune, cell.attributes, cell.foreground, cell.background]
                    .map(u32::to_be_bytes);
                encoded.put_bytes(fields.as_flattened())?;
            }
            current.push(Some(encoded.into_bytes()));
            if links {
                let any = core(engine.row_link_ids(row as i32, &mut ids))?;
                let runs = if any {
                    link_runs(&ids, |id| engine.link_uri(id))
                } else {
                    Vec::new()
                };
                current_links.push(Some(encode_link_runs(&runs)?));
            } else {
                current_links.push(None);
            }
        }
        let movement = search_move
            .then(|| find_move(&self.bytes, &current, width))
            .flatten();
        let mut updates = Vec::new();
        let mut link_updates = Vec::new();
        for (row, bytes) in current.into_iter().enumerate() {
            if !full && !dirty[row] {
                continue;
            }
            let bytes = bytes.unwrap();
            let baseline_row = movement
                .filter(|m| (m.destination..m.destination + m.count).contains(&row))
                .map_or(row, |m| m.source + row - m.destination);
            // Links are compared with the same moved baseline as cells: the view copies link rows
            // with cell rows, so a scroll resends no URIs. A link-only change keeps its own list
            // because it has no differing cell for the span code below.
            if let Some(runs) = current_links[row].take() {
                if full || runs != self.links[baseline_row] {
                    link_updates.push((row, runs));
                }
            }
            if !full && bytes == self.bytes[baseline_row * width..(baseline_row + 1) * width] {
                continue;
            }
            // Compare against the row after any negotiated move. The span includes every changed
            // field of every changed cell, including wide/dummy pairs and wrap/style attributes.
            let span = if !full && features & WIRE_FEATURE_ROW_SPAN != 0 {
                let baseline = &self.bytes[baseline_row * width..(baseline_row + 1) * width];
                let cell = WIRE_CELL_BYTES as usize;
                let first = bytes
                    .chunks_exact(cell)
                    .zip(baseline.chunks_exact(cell))
                    .position(|(a, b)| a != b)
                    .unwrap();
                let last = bytes
                    .chunks_exact(cell)
                    .zip(baseline.chunks_exact(cell))
                    .rposition(|(a, b)| a != b)
                    .unwrap()
                    + 1;
                let range = first * cell..last * cell;
                (range.len() + 4 < width).then_some(range)
            } else {
                None
            };
            updates.push(Update { row, bytes, span });
        }
        if !full
            && movement.is_none()
            && updates.is_empty()
            && link_updates.is_empty()
            && same_frame(frame, self.frame)
            && !reply
        {
            core(engine.clean())?;
            return Ok(Prepared::Unchanged);
        }
        let header = [
            frame.columns,
            frame.rows,
            frame.cursor_x,
            frame.cursor_y,
            frame.scroll,
            frame.alternate,
        ]
        .map(|field| (field as u32).to_be_bytes());
        let move_body = movement
            .map(|m| [m.source, m.destination, m.count].map(|field| (field as u32).to_be_bytes()));
        let mut output = Buffer::default();
        let row_bytes: usize = updates
            .iter()
            .map(|update| {
                HEADER_BYTES
                    + 4
                    + update
                        .span
                        .as_ref()
                        .map_or(update.bytes.len(), |span| 4 + span.len())
            })
            .sum();
        let link_bytes: usize = link_updates
            .iter()
            .map(|(_, runs)| HEADER_BYTES + 4 + runs.len())
            .sum();
        output.reserve(
            2 * HEADER_BYTES
                + link_bytes
                + header.as_flattened().len()
                + row_bytes
                + move_body
                    .as_ref()
                    .map_or(0, |fields| HEADER_BYTES + fields.as_flattened().len()),
        )?;
        output.append_packet_bytes(WIRE_FRAME, tab, header.as_flattened())?;
        if let Some(fields) = move_body {
            output.append_packet_bytes(WIRE_COPY_ROWS, tab, fields.as_flattened())?;
        }
        for update in &updates {
            let row = (update.row as u32).to_be_bytes();
            if let Some(span) = &update.span {
                let column = (span.start as u32 / WIRE_CELL_BYTES).to_be_bytes();
                output.append_packet_parts(
                    WIRE_ROW_SPAN,
                    tab,
                    &[&row, &column, &update.bytes[span.clone()]],
                )?;
            } else {
                output.append_packet_parts(WIRE_ROW, tab, &[&row, &update.bytes])?;
            }
        }
        for (row, runs) in &link_updates {
            output.append_packet_parts(
                WIRE_ROW_LINKS,
                tab,
                &[&(*row as u32).to_be_bytes(), runs],
            )?;
        }
        output.append_packet_bytes(WIRE_FRAME_FINISH, tab, &[])?;
        Ok(Prepared::Changed(Plan {
            frame,
            full,
            movement,
            updates,
            links: links.then_some(link_updates),
            bytes: output.into_bytes(),
        }))
    }

    fn commit(&mut self, plan: Plan, output: &mut Queue, engine: &mut Engine) -> io::Result<()> {
        // This is the only publication point: a rejected queue leaves both damage and baseline intact.
        // replicate owns preparation through commit, excluding feeds/resizes between them.
        output.append_owned(plan.bytes)?;
        let width = plan.frame.columns as usize * WIRE_CELL_BYTES as usize;
        if plan.full {
            self.bytes.resize(width * plan.frame.rows as usize, 0);
        }
        if let Some(m) = plan.movement {
            self.bytes.copy_within(
                m.source * width..(m.source + m.count) * width,
                m.destination * width,
            );
        }
        for update in plan.updates {
            self.bytes[update.row * width..(update.row + 1) * width].copy_from_slice(&update.bytes);
        }
        if let Some(updates) = plan.links {
            if plan.full {
                self.links = vec![Vec::new(); plan.frame.rows as usize];
            }
            if let Some(m) = plan.movement {
                let moved = self.links[m.source..m.source + m.count].to_vec();
                self.links[m.destination..m.destination + m.count].clone_from_slice(&moved);
            }
            for (row, runs) in updates {
                self.links[row] = runs;
            }
        }
        self.frame = plan.frame;
        core(engine.clean())
    }

    pub fn replicate_encoded(
        &mut self,
        engine: &mut Engine,
        output: &mut Queue,
        tab: u32,
        full: bool,
        features: u32,
        reply: bool,
        now: i64,
    ) -> io::Result<Outcome> {
        match self.prepare(engine, tab, full, features, reply, now)? {
            Prepared::Held => {
                if full {
                    self.bytes.clear();
                    self.links.clear();
                }
                Ok(Outcome::Held)
            }
            Prepared::Unchanged => Ok(Outcome::Unchanged),
            Prepared::Changed(plan) => {
                self.commit(plan, output, engine)?;
                Ok(Outcome::Sent)
            }
        }
    }
}
