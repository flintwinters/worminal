//! Encode the shared tab cells and hit regions as a VT strip.
use std::io::{self, Write};
use worminal_protocol::cell::COLOR_DEFAULT_BACKGROUND as BG;
use worminal_workspace::tab_view::{self, Strip, Tab};

#[derive(Default)]
pub(super) struct Bar {
    pub bytes: Vec<u8>,
    pub rows: u32,
    strip: Option<Strip>,
}

impl Bar {
    pub fn hit(&self, column: u32, row: u32, tabs: &[Tab]) -> Option<(u32, bool)> {
        self.strip
            .as_ref()?
            .hit(column as usize, row as usize, tabs)
    }
}

pub(super) fn draw(
    tabs: &[Tab],
    active: u32,
    columns: u32,
    max_rows: u32,
    severity: impl Fn(u32) -> u32,
    palette: &[Option<u32>],
) -> io::Result<Bar> {
    let strip = tab_view::layout(tabs, active, columns as usize, max_rows as usize);
    let mut bytes = Vec::new();
    for row in 1..=strip.rows {
        write!(bytes, "\x1b[{row};1H\x1b[0")?;
        super::screen::color(BG, 48, palette, &mut bytes)?;
        bytes.write_all(b"m\x1b[2K\x1b[0m")?;
    }
    for place in &strip.tabs {
        let tab = &tabs[place.index];
        let style = tab.style(tab.id == active, severity(tab.id));
        write!(bytes, "\x1b[{};{}H", place.row + 1, place.column + 1)?;
        bytes.write_all(b"\x1b[0")?;
        if tab.id == active {
            bytes.write_all(b";1")?;
        }
        super::screen::color(style.foreground, 38, palette, &mut bytes)?;
        super::screen::color(style.background.unwrap_or(BG), 48, palette, &mut bytes)?;
        bytes.write_all(b"m")?;
        let padding = place.width.min(tab.label_column());
        for offset in 0..padding {
            let glyph = if offset == 1 {
                tab.star().unwrap_or(' ')
            } else {
                ' '
            };
            write!(bytes, "{glyph}")?;
        }
        bytes.write_all(&tab.caption().as_bytes()[..place.label_bytes])?;
        if padding < place.width {
            bytes.write_all(b" ")?;
        }
        bytes.write_all(b"\x1b[0m")?;
    }
    if tabs.is_empty() {
        bytes.extend("No terminal tabs".bytes().take(columns as usize));
    }
    Ok(Bar {
        bytes,
        rows: strip.rows as u32,
        strip: Some(strip),
    })
}
