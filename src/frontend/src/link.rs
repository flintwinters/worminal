//! Detect URLs and local paths in visible text for hover and plain left-click.
//! Ported from the retired X11 frontend's url.c; detection reads only the imported viewport.
use std::ops::RangeInclusive;
use worminal_engine::Cell;
use worminal_protocol::cell::{ATTR_WDUMMY, ATTR_WRAP};

const LIMIT: usize = 8192;

fn link_char(rune: u32) -> Option<char> {
    char::from_u32(rune).filter(|character| {
        !character.is_whitespace()
            && !character.is_control()
            && !matches!(character, '<' | '>' | '"' | '\'' | '`')
    })
}
fn url_prefix(text: &[char]) -> bool {
    [
        &['h', 't', 't', 'p', ':', '/', '/'][..],
        &['h', 't', 't', 'p', 's', ':', '/', '/'],
        &['f', 'i', 'l', 'e', ':', '/', '/'],
    ]
    .iter()
    .any(|prefix| text.len() > prefix.len() && text.starts_with(prefix))
}
fn unmatched_closer(text: &[char], open: char, close: char) -> bool {
    text.iter().fold(0i32, |balance, character| {
        balance + i32::from(*character == open) - i32::from(*character == close)
    }) < 0
}

/// Length of a URL candidate after dropping sentence punctuation and unmatched closing brackets.
fn without_trailing_punctuation(text: &[char]) -> usize {
    let mut end = text.len();
    while end > 0 {
        let span = &text[..end];
        let closes = match text[end - 1] {
            '.' | ',' | ';' | ':' | '!' | '?' => true,
            ')' => unmatched_closer(span, '(', ')'),
            ']' => unmatched_closer(span, '[', ']'),
            '}' => unmatched_closer(span, '{', '}'),
            _ => false,
        };
        if !closes {
            break;
        }
        end -= 1;
    }
    end
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
            && link_char(edge.rune).is_some()
            && next
                .get(indent(index + 1))
                .is_some_and(|cell| link_char(cell.rune).is_some())
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
            let dummy = cell.attributes & ATTR_WDUMMY != 0;
            if index == row && position == column {
                clicked = text.len().checked_sub(usize::from(dummy));
            }
            if dummy {
                continue;
            }
            if text.len() == LIMIT - 1 {
                return None;
            }
            text.push(link_char(cell.rune).unwrap_or(' '));
            positions.push(index * columns + position);
        }
        if index == row && column >= width {
            return None;
        }
    }
    let clicked = clicked.filter(|clicked| text.get(*clicked).is_some_and(|c| *c != ' '))?;
    let mut start = clicked;
    while start > 0 && text[start - 1] != ' ' {
        start -= 1;
    }
    let mut end = clicked + 1;
    while end < text.len() && text[end] != ' ' {
        end += 1;
    }
    let mut path_start = start;
    while path_start < end && matches!(text[path_start], '(' | '[' | '{') {
        path_start += 1;
    }
    let path = matches!(&text[path_start..end], ['/', ..] | ['~', '/', ..]);
    if path {
        start = path_start;
    } else {
        // Explicit URLs can follow punctuation or labels. Paths must start the token.
        while start < end && !url_prefix(&text[start..end]) {
            start += 1;
        }
    }
    if start == end || clicked < start {
        return None;
    }
    let end = without_trailing_punctuation(&text[start..end]) + start;
    if clicked >= end {
        return None;
    }
    let target: String = text[start..end].iter().collect();
    let uri = if path {
        let path = match target.strip_prefix("~/") {
            Some(relative) => glib::home_dir().join(relative.trim_start_matches('/')),
            None => target.into(),
        };
        glib::filename_to_uri(path, None).ok()?.into()
    } else {
        target
    };
    let last = positions[end - 1];
    let wide = last % columns + 1 < columns && cells[last + 1].attributes & ATTR_WDUMMY != 0;
    Some((uri, positions[start]..=last + usize::from(wide)))
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
    fn finds_absolute_and_home_relative_paths_at_token_starts() {
        let cells = grid(
            &["see (/var/é#?%.txt), ~/notes relative/not-a-link /"],
            60,
            &[],
        );
        assert_eq!(
            at(&cells, 60, 10, 0),
            Some(("file:///var/%C3%A9%23%3F%25.txt".into(), 5..=17))
        );
        let home_notes = glib::filename_to_uri(glib::home_dir().join("notes"), None).unwrap();
        assert_eq!(url(&cells, 60, 24, 0).as_deref(), Some(home_notes.as_str()));
        assert_eq!(url(&cells, 60, 34, 0), None);
        assert_eq!(url(&cells, 60, 49, 0).as_deref(), Some("file:///"));

        let wrapped = grid(&["/var/long", "-name"], 9, &[0]);
        assert_eq!(
            at(&wrapped, 9, 2, 1),
            Some(("file:///var/long-name".into(), 0..=13))
        );
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
        let cells = grid(&["wrote file:///home/u/a.txt."], 27, &[]);
        assert_eq!(
            url(&cells, 27, 8, 0).as_deref(),
            Some("file:///home/u/a.txt")
        );
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
