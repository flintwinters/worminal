//! Plain left-click on an http(s) URL in visible text opens it with xdg-open; the pointer's
//! link is underlined.
//! Ported from the retired X11 frontend's url.c; detection reads only the imported viewport.
use std::ops::RangeInclusive;
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

/// The URL under (column, row) in a row-major cell grid, following wrapped rows, and the
/// row-major cell indices it covers.
///
/// Full-screen applications (Ink, curses) break long URLs with their own newlines, so those rows
/// carry no ATTR_WRAP. A row whose last column holds a URL character continues into a following
/// row that starts, after any indentation, with a URL character. The continuation's indentation
/// is skipped.
pub fn at(
    cells: &[Cell],
    columns: usize,
    column: usize,
    row: usize,
) -> Option<(String, RangeInclusive<usize>)> {
    let rows = cells.len().checked_div(columns).unwrap_or(0);
    if column >= columns || row >= rows {
        return None;
    }
    let line = |index: usize| &cells[index * columns..(index + 1) * columns];
    let wrapped = |index: usize| line(index)[columns - 1].attributes & ATTR_WRAP != 0;
    let indent = |index: usize| {
        line(index)
            .iter()
            .take_while(|cell| cell.rune == u32::from(b' '))
            .count()
    };
    let broken = |index: usize| {
        let edge = &line(index)[columns - 1];
        let next = line(index + 1);
        edge.attributes & ATTR_WDUMMY == 0
            && url_char(edge.rune)
            && next
                .get(indent(index + 1))
                .is_some_and(|cell| url_char(cell.rune))
    };
    let continues = |index: usize| index + 1 < rows && (wrapped(index) || broken(index));
    let mut first = row;
    while first > 0 && continues(first - 1) {
        first -= 1;
    }
    let mut last = row;
    while continues(last) {
        last += 1;
    }
    let mut text = Vec::new();
    let mut positions = Vec::new();
    let mut clicked = None;
    for index in first..=last {
        let cells = line(index);
        let mut width = columns;
        if !continues(index) {
            while width > 0 && cells[width - 1].rune == u32::from(b' ') {
                width -= 1;
            }
        }
        let skip = if index > first && !wrapped(index - 1) {
            indent(index)
        } else {
            0
        };
        if index == row && column < skip {
            return None;
        }
        for (position, cell) in cells[..width].iter().enumerate().skip(skip) {
            if index == row && position == column {
                clicked = Some(text.len());
            }
            if cell.attributes & ATTR_WDUMMY != 0 {
                continue;
            }
            if text.len() == LIMIT - 1 {
                return None;
            }
            text.push(if url_char(cell.rune) {
                cell.rune as u8
            } else {
                b' '
            });
            positions.push(index * columns + position);
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
    (clicked < end).then(|| {
        let url = String::from_utf8_lossy(&text[start..end]).into_owned();
        (url, positions[start]..=positions[end - 1])
    })
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
                .map(|c| Cell {
                    rune: c as u32,
                    attributes: 0,
                    foreground: 0,
                    background: 0,
                })
                .collect();
            if wrap.contains(&index) {
                line[columns - 1].attributes |= ATTR_WRAP;
            }
            cells.extend(line);
        }
        cells
    }
    fn url(cells: &[Cell], columns: usize, column: usize, row: usize) -> Option<String> {
        at(cells, columns, column, row).map(|(url, _)| url)
    }
    #[test]
    fn finds_urls_trims_punctuation_and_follows_wrapped_rows() {
        let cells = grid(&["see (https://a.example/x_(y)).", "plain text"], 32, &[]);
        assert_eq!(
            url(&cells, 32, 10, 0).as_deref(),
            Some("https://a.example/x_(y)")
        );
        assert_eq!(url(&cells, 32, 0, 0), None);
        assert_eq!(
            url(&cells, 32, 29, 0),
            None,
            "trailing punctuation is not the link"
        );
        assert_eq!(url(&cells, 32, 2, 1), None);
        assert_eq!(url(&cells, 32, 31, 0), None, "blank tail is not the link");
        let cells = grid(&["go http://wrap.exa", "mple/path now"], 18, &[0]);
        assert_eq!(
            url(&cells, 18, 1, 1).as_deref(),
            Some("http://wrap.example/path")
        );
        assert_eq!(
            url(&cells, 18, 5, 0).as_deref(),
            Some("http://wrap.example/path")
        );
        assert_eq!(
            at(&cells, 18, 1, 1).map(|(_, span)| span),
            Some(3..=26),
            "span crosses the wrap"
        );
        let cells = grid(&["go http://brok", "en.example/p ok"], 14, &[]);
        assert_eq!(
            url(&cells, 14, 4, 0).as_deref(),
            Some("http://broken.example/p"),
            "application line break"
        );
        assert_eq!(
            url(&cells, 14, 3, 1).as_deref(),
            Some("http://broken.example/p")
        );
        let cells = grid(&["  http://inde", "  nted.ex now"], 13, &[]);
        assert_eq!(
            url(&cells, 13, 2, 1).as_deref(),
            Some("http://indented.ex"),
            "indented continuation"
        );
        assert_eq!(
            url(&cells, 13, 0, 1),
            None,
            "the continuation's indentation is not the link"
        );
        let cells = grid(&["see http://short.example", "next line"], 30, &[]);
        assert_eq!(
            url(&cells, 30, 6, 0).as_deref(),
            Some("http://short.example"),
            "short rows do not join"
        );
        let cells = grid(&["http:// x"], 9, &[]);
        assert_eq!(url(&cells, 9, 2, 0), None, "a bare scheme is not a link");
    }
}
