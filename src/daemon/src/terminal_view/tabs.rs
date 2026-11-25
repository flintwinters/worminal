//! The tab bar consumes the same ordered catalog and directory labels as the desktop view.
use super::screen::safe_text;
use std::io::{self, Write};
use worminal_protocol::worp::events::{split_directory_field, tab_label, TabRecord};

pub(super) const ROWS: u32 = 1;
const MAX_TAB_COLUMNS: usize = 24;

extern "C" {
    fn wcwidth(character: libc::wchar_t) -> libc::c_int;
}

fn width(character: char) -> usize {
    // The view initializes LC_CTYPE before rendering. A conservative fallback also works in C locale.
    let width = unsafe { wcwidth(character as libc::wchar_t) };
    usize::try_from(width).unwrap_or(character.len_utf8())
}

fn fit(text: &str, columns: usize) -> (&str, usize) {
    let mut used = 0;
    for (index, character) in text.char_indices() {
        let next = used + width(character);
        if next > columns {
            return (&text[..index], used);
        }
        used = next;
    }
    (text, used)
}

pub(super) fn draw(tabs: &[TabRecord], active: u32, columns: u32) -> io::Result<Vec<u8>> {
    let columns = columns as usize;
    let mut output = b"\x1b[1;1H\x1b[0m\x1b[2K".to_vec();
    if tabs.is_empty() {
        output.write_all(fit("No terminal tabs", columns).0.as_bytes())?;
        return Ok(output);
    }
    let active = tabs.iter().position(|tab| tab.id == active);
    let label = |index: usize| {
        let (directory, user) = split_directory_field(&tabs[index].directory);
        let name = safe_text(&tab_label(directory, user));
        let name = if name.is_empty() { "Shell" } else { &name };
        format!("{} {name} ", index + 1)
    };
    // Make room for the selected tab before adding its neighbors.
    let mut start = 0;
    if let Some(active) = active {
        start = active;
        let (_, mut used) = fit(&label(active), MAX_TAB_COLUMNS.min(columns));
        while start > 0 {
            let (_, previous) = fit(&label(start - 1), MAX_TAB_COLUMNS);
            if used + previous + 1 > columns {
                break;
            }
            used += previous + 1;
            start -= 1;
        }
    }
    let mut remaining = columns;
    for index in start..tabs.len() {
        if remaining == 0 {
            break;
        }
        let label = label(index);
        let (text, used) = fit(&label, MAX_TAB_COLUMNS.min(remaining));
        if Some(index) == active {
            output.write_all(b"\x1b[7m")?;
        }
        output.write_all(text.as_bytes())?;
        output.write_all(b"\x1b[0m")?;
        if used < remaining {
            output.write_all(b" ")?;
        }
        remaining = remaining.saturating_sub(used + 1);
    }
    Ok(output)
}
