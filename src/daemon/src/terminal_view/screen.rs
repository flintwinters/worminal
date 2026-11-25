//! Paint complete Worp frames as VT rows. Only full row updates are negotiated.
use super::{tabs, Size};
use std::io::{self, Write};
use worminal_protocol::{
    cell::*,
    worp::events::{Cell, CellBuffer, EventKind, FrameState},
};

#[derive(Default)]
pub(super) struct Screen {
    mode: u32,
    geometry: Option<Size>,
    cursor: Option<FrameState>,
    pending: Option<Frame>,
}

struct Frame {
    state: FrameState,
    rows: Vec<Option<CellBuffer>>,
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid terminal viewport.")
}

pub(super) fn safe_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .filter(|character| !character.is_control())
        .collect()
}

impl Screen {
    pub fn live(&self) -> bool {
        self.geometry.is_some()
    }

    pub fn reset(&mut self, size: Size, output: &mut impl Write) -> io::Result<()> {
        self.modes(0, size, output)?;
        *self = Self::default();
        output.write_all(b"\x1b[?25l")?;
        clear_body(size, output)
    }

    pub fn modes(&mut self, mode: u32, size: Size, output: &mut impl Write) -> io::Result<()> {
        for (bit, on, off) in [
            (MODE_APPCURSOR, "\x1b[?1h", "\x1b[?1l"),
            (MODE_APPKEYPAD, "\x1b=", "\x1b>"),
            (MODE_BRCKTPASTE, "\x1b[?2004h", "\x1b[?2004l"),
            (MODE_REVERSE, "\x1b[?5h", "\x1b[?5l"),
        ] {
            if (self.mode ^ mode) & bit != 0 {
                output.write_all(if mode & bit != 0 { on } else { off }.as_bytes())?;
            }
        }
        let cursor_changed = (self.mode ^ mode) & MODE_HIDE != 0;
        self.mode = mode;
        if cursor_changed {
            self.position_cursor(size, output)?;
        }
        Ok(())
    }

    pub fn position_cursor(&self, size: Size, output: &mut impl Write) -> io::Result<()> {
        if let Some(frame) = self.cursor.filter(|frame| {
            frame.scroll == 0
                && self.mode & MODE_HIDE == 0
                && frame.cursor_x < size.0
                && frame.cursor_y < size.1
        }) {
            write!(
                output,
                "\x1b[{};{}H\x1b[?25h",
                frame.cursor_y + tabs::ROWS + 1,
                frame.cursor_x + 1
            )
        } else {
            output.write_all(b"\x1b[?25l")
        }
    }

    pub fn event(
        &mut self,
        kind: EventKind,
        size: Size,
        output: &mut impl Write,
    ) -> io::Result<()> {
        match kind {
            EventKind::FrameStart(state) => {
                state.validate()?;
                self.pending = Some(Frame {
                    state,
                    rows: vec![None; state.rows as usize],
                });
            }
            EventKind::Row {
                row,
                column: None,
                cells,
            } => {
                let Some(frame) = self.pending.as_mut() else {
                    // Switching away and back can discard a frame split across reads.
                    return Ok(());
                };
                if cells.len() != frame.state.columns as usize {
                    return Err(invalid());
                }
                *frame.rows.get_mut(row as usize).ok_or_else(invalid)? = Some(cells);
            }
            EventKind::Row { .. } | EventKind::CopyRows { .. } => return Err(invalid()),
            EventKind::FrameFinish => {
                // Frames already in flight can describe the old size after a SIGWINCH.
                let Some(frame) = self.pending.take() else {
                    return Ok(());
                };
                self.paint(frame, size, output)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn paint(&mut self, frame: Frame, size: Size, output: &mut impl Write) -> io::Result<()> {
        let geometry = (frame.state.columns, frame.state.rows);
        if self.geometry != Some(geometry) {
            clear_body(size, output)?;
            self.geometry = Some(geometry);
        }
        for (index, row) in frame.rows.into_iter().take(size.1 as usize).enumerate() {
            if let Some(cells) = row {
                write!(output, "\x1b[{};1H", index + tabs::ROWS as usize + 1)?;
                paint_row(cells.iter(), size.0 as usize, output)?;
            }
        }
        self.cursor = Some(frame.state);
        self.position_cursor(size, output)
    }
}

fn clear_body((_, rows): Size, output: &mut impl Write) -> io::Result<()> {
    if rows != 0 {
        write!(output, "\x1b[0m\x1b[{};1H\x1b[J", tabs::ROWS + 1)?;
    }
    Ok(())
}

fn paint_row(
    cells: impl Iterator<Item = Cell>,
    width: usize,
    output: &mut impl Write,
) -> io::Result<()> {
    let mut previous = None;
    for (column, cell) in cells.take(width).enumerate() {
        if cell.attributes & ATTR_WDUMMY != 0 {
            continue;
        }
        let style = (cell.attributes & 0xff, cell.foreground, cell.background);
        if previous != Some(style) {
            attributes(style, output)?;
            previous = Some(style);
        }
        let mut character = char::from_u32(cell.rune)
            .filter(|c| !c.is_control())
            .unwrap_or(' ');
        if cell.attributes & ATTR_WIDE != 0 && column + 1 == width {
            character = ' ';
        }
        write!(output, "{character}")?;
    }
    output.write_all(b"\x1b[0m")
}

fn attributes(
    (flags, foreground, background): (u32, u32, u32),
    output: &mut impl Write,
) -> io::Result<()> {
    output.write_all(b"\x1b[0")?;
    for (flag, code) in [
        (ATTR_BOLD, 1),
        (ATTR_FAINT, 2),
        (ATTR_ITALIC, 3),
        (ATTR_UNDERLINE, 4),
        (ATTR_BLINK, 5),
        (ATTR_REVERSE, 7),
        (ATTR_INVISIBLE, 8),
        (ATTR_STRUCK, 9),
    ] {
        if flags & flag != 0 {
            write!(output, ";{code}")?;
        }
    }
    color(foreground, 38, output)?;
    color(background, 48, output)?;
    output.write_all(b"m")
}

fn color(value: u32, channel: u8, output: &mut impl Write) -> io::Result<()> {
    if value & COLOR_TRUECOLOR != 0 {
        let [_, red, green, blue] = value.to_be_bytes();
        write!(output, ";{channel};2;{red};{green};{blue}")
    } else if value < 256 {
        write!(output, ";{channel};5;{value}")
    } else {
        Ok(())
    }
}
