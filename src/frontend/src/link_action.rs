//! What a clicked link does. Terminal output is untrusted, so a link may do exactly what typed input to
//! the same tab can do, and nothing else: it opens http(s) pages, or types text into that tab. The user
//! chose that a newline in `worminal://input` text executes on a plain click. Every other scheme or
//! action is ignored; nothing arbitrary reaches xdg-open.
use crate::{link, viewport::Viewport};
use std::ops::RangeInclusive;

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Open(String),
    /// Keyboard bytes: sent through the input path, not as a bracketed paste, so Enter executes.
    Input(Vec<u8>),
}

pub struct Target {
    pub uri: String,
    pub span: RangeInclusive<usize>,
    /// An OSC 8 link, whose label may differ from its target, as opposed to a detected URL.
    pub explicit: bool,
}

/// The explicit OSC 8 link under the pointer, else a URL detected in the visible text.
pub fn target_at(viewport: &Viewport, column: usize, row: usize) -> Option<Target> {
    if let Some((uri, span)) = viewport.link_at(column, row) {
        return Some(Target {
            uri: uri.to_owned(),
            span,
            explicit: true,
        });
    }
    let columns = viewport.frame().columns as usize;
    link::at(viewport.cells(), columns, column, row).map(|(uri, span)| Target {
        uri,
        span,
        explicit: false,
    })
}

fn hex(byte: u8) -> Option<u8> {
    char::from(byte).to_digit(16).map(|digit| digit as u8)
}

fn percent_decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = hex(*bytes.get(index + 1)?)?;
            let low = hex(*bytes.get(index + 2)?)?;
            decoded.push(high << 4 | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    Some(decoded)
}

/// Non-empty UTF-8 without control characters (C0, DEL, C1) except newline, which becomes Enter.
fn typed_text(encoded: &str) -> Option<Vec<u8>> {
    let text = String::from_utf8(percent_decode(encoded)?).ok()?;
    if text.is_empty() || text.chars().any(|c| c != '\n' && c.is_control()) {
        return None;
    }
    Some(text.replace('\n', "\r").into_bytes())
}

pub fn resolve(target: &str) -> Option<Action> {
    if target.starts_with("http://") || target.starts_with("https://") {
        return Some(Action::Open(target.to_owned()));
    }
    let query = target.strip_prefix("worminal://input?")?;
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix("text="))
        .and_then(typed_text)
        .map(Action::Input)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(target: &str) -> Option<Vec<u8>> {
        match resolve(target) {
            Some(Action::Input(bytes)) => Some(bytes),
            _ => None,
        }
    }

    #[test]
    fn web_links_open_and_other_schemes_are_ignored() {
        assert_eq!(
            resolve("https://example.org/a?b=c"),
            Some(Action::Open("https://example.org/a?b=c".into()))
        );
        assert_eq!(resolve("http://h"), Some(Action::Open("http://h".into())));
        for target in [
            "file:///etc/passwd",
            "ssh://host",
            "javascript:alert(1)",
            "HTTP://upper",
            "worminal://other?text=x",
            "worminal://input",
            "worminal://input?other=x",
            "worminal://input?text=",
            "",
        ] {
            assert_eq!(resolve(target), None, "{target}");
        }
    }

    #[test]
    fn input_text_is_percent_decoded_and_newline_becomes_enter() {
        assert_eq!(
            input("worminal://input?text=ls%20-l"),
            Some(b"ls -l".to_vec())
        );
        assert_eq!(input("worminal://input?text=a+b"), Some(b"a+b".to_vec()));
        assert_eq!(input("worminal://input?text=ls%0A"), Some(b"ls\r".to_vec()));
        assert_eq!(
            input("worminal://input?x=1&text=%E2%9C%93%0Aok"),
            Some("\u{2713}\rok".as_bytes().to_vec())
        );
    }

    #[test]
    fn controls_and_malformed_text_make_the_link_ignored() {
        for text in [
            "%1B%5B31m", // ESC
            "a%00b",     // NUL
            "%0D",       // CR
            "%09",       // TAB
            "%7F",       // DEL
            "%C2%9B31m", // C1 CSI
            "%FF",       // not UTF-8
            "%",         // truncated escape
            "%G0",       // bad hex
            "ok%1b",
        ] {
            assert_eq!(
                input(&format!("worminal://input?text={text}")),
                None,
                "{text}"
            );
        }
    }
}
