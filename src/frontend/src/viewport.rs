//! One imported viewport. Frame deltas remain bounded and private until FRAME_FINISH.
use std::{io, ops::RangeInclusive, rc::Rc};
use worminal_engine::{Cell, Frame, Selection, SelectionRange};
use worminal_engine_sys::{
    ATTR_WIDE, ATTR_WRAP, WORMINAL_DEFAULT_BACKGROUND, WORMINAL_DEFAULT_FOREGROUND,
};
use worminal_protocol::*;

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid imported terminal frame",
    )
}
fn engine<T>(result: Result<T, worminal_engine::Error>) -> io::Result<T> {
    result.map_err(|error| io::Error::other(format!("selection engine: {}", error.0)))
}
fn blank() -> Cell {
    Cell {
        rune: u32::from(' '),
        attributes: 0,
        foreground: WORMINAL_DEFAULT_FOREGROUND,
        background: WORMINAL_DEFAULT_BACKGROUND,
    }
}
fn dimensions(columns: u32, rows: u32) -> io::Result<()> {
    if columns == 0 || columns > WORKSPACE_MAX_COLS || rows == 0 || rows > WORKSPACE_MAX_ROWS {
        Err(invalid())
    } else {
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Damage {
    pub full: bool,
    pub rows: Vec<usize>,
}
impl Damage {
    pub fn changed(&self) -> bool {
        self.full || !self.rows.is_empty()
    }
}

enum Operation {
    Span {
        row: usize,
        column: usize,
        cells: Vec<Cell>,
    },
    Copy {
        source: usize,
        destination: usize,
        count: usize,
    },
    Links {
        row: usize,
        runs: Rc<[LinkRun]>,
    },
}
struct Pending {
    frame: Frame,
    operations: Vec<Operation>,
    bytes: usize,
}
impl Pending {
    fn row_links(&mut self, mut body: Payload) -> io::Result<()> {
        let row = body.u32()? as usize;
        if row >= self.frame.rows as usize {
            return Err(invalid());
        }
        let runs = decode_link_runs(body, self.frame.columns as u32)?;
        let payload = runs
            .iter()
            .map(|run| std::mem::size_of::<LinkRun>() + run.uri.len())
            .sum();
        self.push(
            Operation::Links {
                row,
                runs: runs.into(),
            },
            payload,
        )
    }
    fn push(&mut self, operation: Operation, payload: usize) -> io::Result<()> {
        let bytes = self
            .bytes
            .checked_add(std::mem::size_of::<Operation>() + payload)
            .ok_or_else(invalid)?;
        if bytes > MAX_PAYLOAD {
            return Err(invalid());
        }
        self.operations.push(operation);
        self.bytes = bytes;
        Ok(())
    }
}

pub struct Viewport {
    frame: Frame,
    cells: Vec<Cell>,
    /// Link runs per row, allocated when the first non-empty run list arrives. Rc keeps row copies cheap.
    links: Vec<Rc<[LinkRun]>>,
    pending: Option<Pending>,
    live: bool,
    selection: Option<Selection>,
    selected: Vec<SelectionRange>,
}

impl Viewport {
    pub fn new(columns: u32, rows: u32) -> io::Result<Self> {
        dimensions(columns, rows)?;
        Ok(Self {
            frame: Frame {
                columns: columns as i32,
                rows: rows as i32,
                ..Frame::default()
            },
            cells: Vec::new(),
            links: Vec::new(),
            pending: None,
            live: false,
            selection: None,
            selected: Vec::new(),
        })
    }
    pub fn frame(&self) -> Frame {
        self.frame
    }
    pub fn live(&self) -> bool {
        self.live
    }
    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }
    pub fn row(&self, row: usize) -> Option<&[Cell]> {
        if row >= self.frame.rows as usize || self.cells.is_empty() {
            return None;
        }
        let first = row * self.frame.columns as usize;
        Some(&self.cells[first..first + self.frame.columns as usize])
    }
    pub fn selected(&self, column: usize, row: usize) -> bool {
        self.selected
            .get(row)
            .is_some_and(|range| column >= range.first as usize && column < range.end as usize)
    }
    /// Explicit OSC 8 target under a cell and the row-major cells its run covers.
    pub fn link_at(&self, column: usize, row: usize) -> Option<(&str, RangeInclusive<usize>)> {
        let columns = self.frame.columns as usize;
        let run = self.links.get(row)?.iter().find(|run| {
            (run.column as usize..(run.column + run.count) as usize).contains(&column)
        })?;
        let first = row * columns + run.column as usize;
        Some((&run.uri, first..=first + run.count as usize - 1))
    }
    pub fn release(&mut self) {
        self.live = false;
        self.pending = None;
        self.links.clear();
    }

    pub fn packet(&mut self, packet: &Packet) -> io::Result<Option<Damage>> {
        let result = self.apply(packet);
        // Malformed/incomplete deltas can never corrupt the last completely imported frame.
        if result.is_err() {
            self.pending = None;
        }
        result
    }

    fn apply(&mut self, packet: &Packet) -> io::Result<Option<Damage>> {
        let mut body = Payload::new(&packet.body);
        match packet.kind {
            WIRE_FRAME => {
                if self.pending.is_some() {
                    return Err(invalid());
                }
                let columns = body.u32()?;
                let rows = body.u32()?;
                dimensions(columns, rows)?;
                let cursor_x = body.u32()?;
                let cursor_y = body.u32()?;
                let scroll = body.u32()?;
                let alternate = body.u32()?;
                body.finish()?;
                if cursor_x >= columns
                    || cursor_y >= rows
                    || scroll > i32::MAX as u32
                    || alternate > 1
                {
                    return Err(invalid());
                }
                self.pending = Some(Pending {
                    frame: Frame {
                        columns: columns as i32,
                        rows: rows as i32,
                        cursor_x: cursor_x as i32,
                        cursor_y: cursor_y as i32,
                        scroll: scroll as i32,
                        alternate: alternate as i32,
                        redraw_deadline: 0,
                    },
                    operations: Vec::new(),
                    bytes: 0,
                });
            }
            WIRE_ROW | WIRE_ROW_SPAN => {
                let pending = self.pending.as_mut().ok_or_else(invalid)?;
                let row = body.u32()? as usize;
                if row >= pending.frame.rows as usize {
                    return Err(invalid());
                }
                let columns = pending.frame.columns as usize;
                let span = packet.kind == WIRE_ROW_SPAN;
                let column = if span { body.u32()? as usize } else { 0 };
                let header = if span { 8 } else { 4 };
                let length = packet.body.len().checked_sub(header).ok_or_else(invalid)?;
                let count = length / WIRE_CELL_BYTES as usize;
                if count == 0
                    || count > columns
                    || column > columns - count
                    || length % WIRE_CELL_BYTES as usize != 0
                    || (!span && count != columns)
                {
                    return Err(invalid());
                }
                let mut cells = Vec::with_capacity(count);
                for _ in 0..count {
                    cells.push(Cell {
                        rune: body.u32()?,
                        attributes: body.u32()? & u32::from(u16::MAX),
                        foreground: body.u32()?,
                        background: body.u32()?,
                    });
                }
                body.finish()?;
                pending.push(
                    Operation::Span { row, column, cells },
                    count * std::mem::size_of::<Cell>(),
                )?;
            }
            WIRE_COPY_ROWS => {
                let pending = self.pending.as_mut().ok_or_else(invalid)?;
                let source = body.u32()? as usize;
                let destination = body.u32()? as usize;
                let count = body.u32()? as usize;
                body.finish()?;
                let rows = pending.frame.rows as usize;
                if count == 0 || count > rows || source > rows - count || destination > rows - count
                {
                    return Err(invalid());
                }
                pending.push(
                    Operation::Copy {
                        source,
                        destination,
                        count,
                    },
                    // Bound work as well as storage. Tiny move packets must not defer hundreds
                    // of gigabytes of copying into one GTK callback at FRAME_FINISH.
                    count * pending.frame.columns as usize * std::mem::size_of::<Cell>(),
                )?;
            }
            WIRE_ROW_LINKS => self.pending.as_mut().ok_or_else(invalid)?.row_links(body)?,
            WIRE_FRAME_FINISH => {
                body.finish()?;
                return self.finish().map(Some);
            }
            _ => {}
        }
        Ok(None)
    }

    fn finish(&mut self) -> io::Result<Damage> {
        let pending = self.pending.take().ok_or_else(invalid)?;
        let old = self.frame;
        let frame = pending.frame;
        let columns = frame.columns as usize;
        let rows = frame.rows as usize;
        let geometry = old.columns != frame.columns || old.rows != frame.rows;
        let full = self.cells.is_empty()
            || geometry
            || old.scroll != frame.scroll
            || old.alternate != frame.alternate
            || !self.live;
        // Configure the coordinate-only C selection before publishing new cells; it can reject no valid wire frame.
        if let Some(selection) = &mut self.selection {
            engine(selection.configure(&frame))?;
        }
        if self.cells.is_empty() || geometry {
            let mut resized = vec![blank(); columns * rows];
            for row in 0..rows.min(old.rows as usize) {
                if let Some(source) = self.row(row) {
                    let count = columns.min(source.len());
                    let destination = &mut resized[row * columns..(row + 1) * columns];
                    destination[..count].copy_from_slice(&source[..count]);
                    if old.columns != frame.columns {
                        destination[count - 1].attributes &= !ATTR_WRAP;
                    }
                    if destination[columns - 1].attributes & ATTR_WIDE != 0 {
                        destination[columns - 1] = blank();
                    }
                }
            }
            self.cells = resized;
            self.links.clear();
        }
        let mut damage = vec![false; rows];
        if old.cursor_x != frame.cursor_x || old.cursor_y != frame.cursor_y {
            if let Some(row) = damage.get_mut(old.cursor_y as usize) {
                *row = true;
            }
            damage[frame.cursor_y as usize] = true;
        }
        for operation in pending.operations {
            match operation {
                Operation::Span { row, column, cells } => {
                    let first = row * columns + column;
                    let target = &mut self.cells[first..first + cells.len()];
                    if target != cells {
                        target.copy_from_slice(&cells);
                        damage[row] = true;
                    }
                }
                Operation::Copy {
                    source,
                    destination,
                    count,
                } => {
                    // copy_within has the protocol's memmove semantics, including downward overlap.
                    self.cells.copy_within(
                        source * columns..(source + count) * columns,
                        destination * columns,
                    );
                    damage[destination..destination + count].fill(true);
                    if !self.links.is_empty() {
                        let moved = self.links[source..source + count].to_vec();
                        self.links[destination..destination + count].clone_from_slice(&moved);
                    }
                }
                Operation::Links { row, runs } => {
                    let current = self.links.get(row).map_or(&[][..], |runs| &runs[..]);
                    if *current != *runs {
                        if self.links.is_empty() {
                            self.links = vec![Rc::from([]); rows];
                        }
                        self.links[row] = runs;
                        damage[row] = true;
                    }
                }
            }
        }
        self.frame = frame;
        self.live = true;
        if let Some(selection) = &mut self.selection {
            engine(selection.ranges(&mut self.selected))?;
        }
        Ok(Damage {
            full,
            rows: damage
                .iter()
                .enumerate()
                .filter_map(|(row, dirty)| dirty.then_some(row))
                .collect(),
        })
    }

    pub fn select(
        &mut self,
        column: i32,
        row: i32,
        start: bool,
        snap: i32,
        rectangular: bool,
        done: bool,
    ) -> io::Result<Damage> {
        if self.cells.is_empty() {
            self.cells = vec![blank(); self.frame.columns as usize * self.frame.rows as usize];
        }
        if self.selection.is_none() {
            self.selection = Some(engine(Selection::new(&self.frame))?);
        }
        let selection = self.selection.as_mut().unwrap();
        engine(selection.select(&self.cells, column, row, start, snap, rectangular, done))?;
        self.selection_damage()
    }
    pub fn select_all(&mut self) -> io::Result<Damage> {
        self.select(0, 0, true, 2, false, false)?;
        self.select(
            self.frame.columns - 1,
            self.frame.rows - 1,
            false,
            0,
            false,
            true,
        )
    }
    pub fn clear_selection(&mut self) -> io::Result<Damage> {
        if let Some(selection) = &mut self.selection {
            engine(selection.clear())?;
        }
        self.selection_damage()
    }
    fn selection_damage(&mut self) -> io::Result<Damage> {
        let mut ranges = Vec::new();
        if let Some(selection) = &mut self.selection {
            engine(selection.ranges(&mut ranges))?;
        }
        let rows = (0..self.frame.rows as usize)
            .filter(|row| self.selected.get(*row) != ranges.get(*row))
            .collect();
        self.selected = ranges;
        Ok(Damage { full: false, rows })
    }
    pub fn selection_text(&mut self) -> io::Result<Vec<u8>> {
        match &mut self.selection {
            Some(selection) => engine(selection.text(&self.cells)),
            None => Ok(Vec::new()),
        }
    }
}
