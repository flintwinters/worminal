//! One imported viewport. Frame deltas remain bounded and private until FRAME_FINISH.
use std::io;
use worminal_engine::{Cell, Frame, Selection, SelectionRange};
use worminal_engine_sys::{
    ATTR_WIDE, ATTR_WRAP, WORMINAL_DEFAULT_BACKGROUND, WORMINAL_DEFAULT_FOREGROUND,
};
use worminal_protocol::worp::events::{Event, EventKind};
use worminal_protocol::worp::v2_events;
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
}
struct Pending {
    frame: Frame,
    operations: Vec<Operation>,
    bytes: usize,
}
impl Pending {
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
    pub fn release(&mut self) {
        self.live = false;
        self.pending = None;
    }

    /// Compatibility entry point for consumers of the version-2 binding.
    pub fn packet(&mut self, packet: &Packet) -> io::Result<Option<Damage>> {
        let decoded = v2_events::decode(Packet {
            kind: packet.kind,
            tab: packet.tab,
            body: packet.body.clone(),
        });
        match decoded {
            Ok(Some(event)) => self.event(&event),
            Ok(None) => Ok(None),
            Err(error) => {
                self.pending = None;
                Err(error)
            }
        }
    }

    pub fn event(&mut self, event: &Event) -> io::Result<Option<Damage>> {
        let result = self.apply(event);
        // Malformed/incomplete deltas can never corrupt the last completely imported frame.
        if result.is_err() {
            self.pending = None;
        }
        result
    }

    fn apply(&mut self, event: &Event) -> io::Result<Option<Damage>> {
        match &event.kind {
            EventKind::FrameStart(frame) => {
                if self.pending.is_some() {
                    return Err(invalid());
                }
                frame.validate()?;
                self.pending = Some(Pending {
                    frame: Frame {
                        columns: frame.columns as i32,
                        rows: frame.rows as i32,
                        cursor_x: frame.cursor_x as i32,
                        cursor_y: frame.cursor_y as i32,
                        scroll: frame.scroll as i32,
                        alternate: i32::from(frame.alternate),
                        redraw_deadline: 0,
                    },
                    operations: Vec::new(),
                    bytes: 0,
                });
            }
            EventKind::Row { row, column, cells } => {
                let pending = self.pending.as_mut().ok_or_else(invalid)?;
                let row = *row as usize;
                let columns = pending.frame.columns as usize;
                let first = column.unwrap_or(0) as usize;
                let count = cells.len();
                if row >= pending.frame.rows as usize
                    || count == 0
                    || count > columns
                    || first > columns - count
                    || (column.is_none() && count != columns)
                {
                    return Err(invalid());
                }
                let cells = cells
                    .iter()
                    .map(|cell| Cell {
                        rune: cell.rune,
                        attributes: cell.attributes & u32::from(u16::MAX),
                        foreground: cell.foreground,
                        background: cell.background,
                    })
                    .collect();
                pending.push(
                    Operation::Span {
                        row,
                        column: first,
                        cells,
                    },
                    count * std::mem::size_of::<Cell>(),
                )?;
            }
            EventKind::CopyRows {
                source,
                destination,
                count,
            } => {
                let pending = self.pending.as_mut().ok_or_else(invalid)?;
                let (source, destination, count) =
                    (*source as usize, *destination as usize, *count as usize);
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
                    // Bound work as well as storage: tiny moves must not defer unbounded copying.
                    count * pending.frame.columns as usize * std::mem::size_of::<Cell>(),
                )?;
            }
            EventKind::FrameFinish => return self.finish().map(Some),
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
