//! A viewport baseline describes queued bytes, never merely observed engine state.
use crate::{
    queue::Queue,
    state::{core, Screen},
};
use std::io;
use worminal_engine::{Cell, Engine, Frame};
use worminal_protocol::{
    Buffer, Packet, WIRE_CELL_BYTES, WIRE_COPY_ROWS, WIRE_FRAME, WIRE_FRAME_FINISH, WIRE_ROW,
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

fn packet(output: &mut Buffer, kind: u32, tab: u32, body: Buffer) -> io::Result<()> {
    output.append_packet(&Packet {
        kind,
        tab,
        body: body.into_bytes(),
    })
}

struct Plan {
    frame: Frame,
    full: bool,
    movement: Option<Move>,
    updates: Vec<(usize, Vec<u8>)>,
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
    fn prepare(
        &self,
        engine: &mut Engine,
        tab: u32,
        full: bool,
        copy_rows: bool,
        reply: bool,
        now: i64,
    ) -> io::Result<Prepared> {
        let frame = core(engine.frame(now))?;
        if frame.redraw_deadline != 0 {
            return Ok(Prepared::Held);
        }
        let width = frame.columns as usize * WIRE_CELL_BYTES as usize;
        let rows = frame.rows as usize;
        let full = full || self.frame.columns != frame.columns || self.bytes.len() != width * rows;
        let dirty: Vec<_> = (0..frame.rows)
            .map(|row| core(engine.row_dirty(row)))
            .collect::<io::Result<_>>()?;
        let dirty_count = dirty.iter().filter(|flag| **flag).count();
        let search_move = !full && copy_rows && dirty_count >= 3 && dirty_count * 2 >= rows;
        let mut current = Vec::with_capacity(rows);
        let mut cells: Vec<Cell> = Vec::new();
        for (row, dirty) in dirty.iter().enumerate() {
            if !full && !search_move && !dirty {
                current.push(None);
                continue;
            }
            core(engine.row(row as i32, &mut cells))?;
            let mut encoded = Buffer::default();
            for cell in &cells {
                for value in [cell.rune, cell.attributes, cell.foreground, cell.background] {
                    encoded.put_u32(value)?;
                }
            }
            current.push(Some(encoded.into_bytes()));
        }
        let movement = search_move
            .then(|| find_move(&self.bytes, &current, width))
            .flatten();
        let mut output = Buffer::default();
        let mut header = Buffer::default();
        for field in [
            frame.columns,
            frame.rows,
            frame.cursor_x,
            frame.cursor_y,
            frame.scroll,
            frame.alternate,
        ] {
            header.put_u32(field as u32)?;
        }
        packet(&mut output, WIRE_FRAME, tab, header)?;
        if let Some(m) = movement {
            let mut body = Buffer::default();
            for field in [m.source, m.destination, m.count] {
                body.put_u32(field as u32)?;
            }
            packet(&mut output, WIRE_COPY_ROWS, tab, body)?;
        }
        let mut updates = Vec::new();
        for (row, bytes) in current.into_iter().enumerate() {
            if !full && !dirty[row] {
                continue;
            }
            let bytes = bytes.unwrap();
            let baseline_row = movement
                .filter(|m| (m.destination..m.destination + m.count).contains(&row))
                .map_or(row, |m| m.source + row - m.destination);
            if !full && bytes == self.bytes[baseline_row * width..(baseline_row + 1) * width] {
                continue;
            }
            let mut body = Buffer::default();
            body.put_u32(row as u32)?;
            body.put_bytes(&bytes)?;
            packet(&mut output, WIRE_ROW, tab, body)?;
            updates.push((row, bytes));
        }
        if !full
            && movement.is_none()
            && updates.is_empty()
            && same_frame(frame, self.frame)
            && !reply
        {
            core(engine.clean())?;
            return Ok(Prepared::Unchanged);
        }
        packet(&mut output, WIRE_FRAME_FINISH, tab, Buffer::default())?;
        Ok(Prepared::Changed(Plan {
            frame,
            full,
            movement,
            updates,
            bytes: output.into_bytes(),
        }))
    }

    fn commit(&mut self, plan: Plan, output: &mut Queue, engine: &mut Engine) -> io::Result<()> {
        // This is the only publication point: a rejected queue leaves both damage and baseline intact.
        // replicate owns preparation through commit, excluding feeds/resizes between them.
        output.append(&plan.bytes)?;
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
        for (row, bytes) in plan.updates {
            self.bytes[row * width..(row + 1) * width].copy_from_slice(&bytes);
        }
        self.frame = plan.frame;
        core(engine.clean())
    }

    pub fn replicate(
        &mut self,
        engine: &mut Engine,
        output: &mut Queue,
        tab: u32,
        full: bool,
        copy_rows: bool,
        reply: bool,
        now: i64,
    ) -> io::Result<Outcome> {
        match self.prepare(engine, tab, full, copy_rows, reply, now)? {
            Prepared::Held => {
                if full {
                    self.bytes.clear();
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
