//! Plain left-click on an http(s) URL in visible text opens it with xdg-open.
//! Ported from the retired X11 frontend's url.c; detection reads only the imported viewport.
use worminal_engine::Cell;
use worminal_engine_sys::{ATTR_WDUMMY, ATTR_WRAP};

const LIMIT: usize = 8192;

fn url_char(rune: u32) -> bool {
    rune > 32 && rune < 127 && !matches!(rune as u8, b'<' | b'>' | b'"' | b'\'' | b'`')
}
fn url_prefix(text: &[u8]) -> bool {
    [&b"http://"[..], b"https://"].iter().any(|prefix| {
        text.len() > prefix.len() && text.starts_with(prefix) && text[prefix.len()] != b' '
    })
}
fn unmatched_closer(text: &[u8], open: u8, close: u8) -> bool {
    text.iter().fold(0i32, |balance, byte| {
        balance + i32::from(*byte == open) - i32::from(*byte == close)
    }) < 0
}

/// The URL under (column, row) in a row-major cell grid, following soft-wrapped rows.
pub fn at(cells: &[Cell], columns: usize, column: usize, row: usize) -> Option<String> {
    let rows = if columns == 0 { 0 } else { cells.len() / columns };
    if column >= columns || row >= rows {
        return None;
    }
    let line = |index: usize| &cells[index * columns..(index + 1) * columns];
    let wrapped = |index: usize| line(index)[columns - 1].attributes & ATTR_WRAP != 0;
    let mut first = row;
    while first > 0 && wrapped(first - 1) {
        first -= 1;
    }
    let mut last = row;
    while last + 1 < rows && wrapped(last) {
        last += 1;
    }
    let mut text = Vec::new();
    let mut clicked = None;
    for index in first..=last {
        let cells = line(index);
        let mut width = columns;
        if !wrapped(index) {
            while width > 0 && cells[width - 1].rune == u32::from(b' ') {
                width -= 1;
            }
        }
        for (position, cell) in cells[..width].iter().enumerate() {
            if index == row && position == column {
                clicked = Some(text.len());
            }
            if cell.attributes & ATTR_WDUMMY != 0 {
                continue;
            }
            if text.len() == LIMIT - 1 {
                return None;
            }
            text.push(if url_char(cell.rune) { cell.rune as u8 } else { b' ' });
        }
        if index == row && column >= width {
            return None;
        }
    }
    let clicked = clicked.filter(|clicked| text.get(*clicked).is_some_and(|byte| *byte != b' '))?;
    let mut start = clicked;
    while start > 0 && text[start - 1] != b' ' {
        start -= 1;
    }
    let mut end = clicked + 1;
    while end < text.len() && text[end] != b' ' {
        end += 1;
    }
    // A URL may follow punctuation such as an opening parenthesis.
    while start < end && !url_prefix(&text[start..end]) {
        start += 1;
    }
    if start == end || clicked < start {
        return None;
    }
    while end > start {
        let span = &text[start..end];
        let closes = match text[end - 1] {
            b'.' | b',' | b';' | b':' | b'!' | b'?' => true,
            b')' => unmatched_closer(span, b'(', b')'),
            b']' => unmatched_closer(span, b'[', b']'),
            b'}' => unmatched_closer(span, b'{', b'}'),
            _ => false,
        };
        if !closes {
            break;
        }
        end -= 1;
    }
    (clicked < end).then(|| String::from_utf8_lossy(&text[start..end]).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn grid(rows: &[&str], columns: usize, wrap: &[usize]) -> Vec<Cell> {
        let mut cells = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            let mut line: Vec<Cell> = row
                .chars()
                .chain(std::iter::repeat(' '))
                .take(columns)
                .map(|c| Cell { rune: c as u32, attributes: 0, foreground: 0, background: 0 })
                .collect();
            if wrap.contains(&index) {
                line[columns - 1].attributes |= ATTR_WRAP;
            }
            cells.extend(line);
        }
        cells
    }
    #[test]
    fn finds_urls_trims_punctuation_and_follows_soft_wraps() {
        let cells = grid(&["see (https://a.example/x_(y)).", "plain text"], 32, &[]);
        assert_eq!(at(&cells, 32, 10, 0).as_deref(), Some("https://a.example/x_(y)"));
        assert_eq!(at(&cells, 32, 0, 0), None);
        assert_eq!(at(&cells, 32, 29, 0), None, "trailing punctuation is not the link");
        assert_eq!(at(&cells, 32, 2, 1), None);
        assert_eq!(at(&cells, 32, 31, 0), None, "blank tail is not the link");
        let cells = grid(&["go http://wrap.exa", "mple/path now"], 18, &[0]);
        assert_eq!(at(&cells, 18, 1, 1).as_deref(), Some("http://wrap.example/path"));
        assert_eq!(at(&cells, 18, 5, 0).as_deref(), Some("http://wrap.example/path"));
        let cells = grid(&["http:// x"], 9, &[]);
        assert_eq!(at(&cells, 9, 2, 0), None, "a bare scheme is not a link");
    }
}
