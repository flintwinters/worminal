//! Worp state changes and viewport deltas, independent of framing and communication medium.
use super::Reply;
use crate::{WIRE_CELL_BYTES, WORKSPACE_MAX_COLS, WORKSPACE_MAX_ROWS};
use std::{fmt, io};

pub const MAX_CATALOG_TABS: usize = 4096;
pub const MAX_CATALOG_COLORS: usize = 270;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub tab: u32,
    pub kind: EventKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventKind {
    Catalog(Catalog),
    FrameStart(FrameState),
    Row {
        row: u32,
        column: Option<u32>,
        cells: CellBuffer,
    },
    CopyRows {
        source: u32,
        destination: u32,
        count: u32,
    },
    FrameFinish,
    Clipboard(Vec<u8>),
    Bell,
    Print(Vec<u8>),
    Released,
    Error(Vec<u8>),
    Created,
    Capabilities(u32),
    ServiceInfo(u32),
    CommandDone,
    Selection {
        generation: u32,
        done: bool,
        text: Vec<u8>,
    },
    Reply(Reply),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Catalog {
    pub closed: u32,
    pub fallback: u32,
    pub tabs: Vec<TabRecord>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabKind {
    Terminal,
    Browser,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabRecord {
    pub id: u32,
    pub kind: TabKind,
    pub url: Vec<u8>,
    pub mode: u32,
    pub cursor: u32,
    pub title: Vec<u8>,
    pub directory: Vec<u8>,
    pub colors: Vec<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameState {
    pub columns: u32,
    pub rows: u32,
    pub cursor_x: u32,
    pub cursor_y: u32,
    pub scroll: u32,
    pub alternate: bool,
}
impl FrameState {
    pub fn validate(&self) -> io::Result<()> {
        if self.columns == 0
            || self.columns > WORKSPACE_MAX_COLS
            || self.rows == 0
            || self.rows > WORKSPACE_MAX_ROWS
            || self.cursor_x >= self.columns
            || self.cursor_y >= self.rows
            || self.scroll > i32::MAX as u32
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid Worp viewport geometry.",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub rune: u32,
    pub attributes: u32,
    pub foreground: u32,
    pub background: u32,
}

/// Canonical packed cells own their incoming allocation. A row header can precede the cells without a copy.
#[derive(Clone)]
pub struct CellBuffer {
    bytes: Vec<u8>,
    offset: usize,
}
impl CellBuffer {
    pub fn new(bytes: Vec<u8>) -> io::Result<Self> {
        Self::from_payload(bytes, 0)
    }
    pub(super) fn from_payload(bytes: Vec<u8>, offset: usize) -> io::Result<Self> {
        let length = bytes
            .len()
            .checked_sub(offset)
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))?;
        if length == 0
            || length % WIRE_CELL_BYTES as usize != 0
            || length / WIRE_CELL_BYTES as usize > WORKSPACE_MAX_COLS as usize
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid Worp cell buffer.",
            ));
        }
        Ok(Self { bytes, offset })
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[self.offset..]
    }
    pub fn len(&self) -> usize {
        self.as_bytes().len() / WIRE_CELL_BYTES as usize
    }
    pub fn is_empty(&self) -> bool {
        self.as_bytes().is_empty()
    }
    pub fn iter(&self) -> impl Iterator<Item = Cell> + '_ {
        self.as_bytes()
            .chunks_exact(WIRE_CELL_BYTES as usize)
            .map(|bytes| {
                let number =
                    |first| u32::from_be_bytes(bytes[first..first + 4].try_into().unwrap());
                Cell {
                    rune: number(0),
                    attributes: number(4),
                    foreground: number(8),
                    background: number(12),
                }
            })
    }
}
impl PartialEq for CellBuffer {
    fn eq(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}
impl Eq for CellBuffer {}
impl fmt::Debug for CellBuffer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CellBuffer")
            .field("cells", &self.len())
            .finish()
    }
}

/// Borrowed producer operations avoid constructing an owned Event for every painted row.
pub enum FrameRef<'a> {
    Start(FrameState),
    Row {
        row: u32,
        column: Option<u32>,
        cells: &'a [u8],
    },
    CopyRows {
        source: u32,
        destination: u32,
        count: u32,
    },
    Finish,
}
