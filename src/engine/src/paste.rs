//! In-place paste normalization, independent of the terminal parser and transport.
use worminal_protocol::{cell::MODE_BRCKTPASTE, input::MAX_PASTE_BYTES};

pub struct PasteFilter {
    bracketed: bool,
    previous_cr: bool,
}
impl PasteFilter {
    pub fn new(bracketed: bool) -> Self {
        Self {
            bracketed,
            previous_cr: false,
        }
    }
    pub fn filter(&mut self, bytes: &mut [u8]) -> usize {
        let mut written = 0;
        for i in 0..bytes.len() {
            let mut byte = bytes[i];
            // ESC can close the bracketed envelope; ETX can abort it in shells.
            // Ordinary paste folds CRLF/LF even when CRLF straddles input chunks.
            if self.bracketed && matches!(byte, 0x1b | 0x03) {
                continue;
            }
            if !self.bracketed && byte == b'\n' {
                let paired = self.previous_cr;
                self.previous_cr = false;
                if paired {
                    continue;
                }
                byte = b'\r';
            } else {
                self.previous_cr = byte == b'\r';
            }
            bytes[written] = byte;
            written += 1;
        }
        written
    }
}
/// Filters one paste chunk for a terminal in `mode`, bracketing it at `begin`/`end` when the
/// application asked for bracketed paste.
pub fn envelope(bytes: &[u8], begin: bool, end: bool, mode: u32) -> Vec<u8> {
    let bracketed = mode & MODE_BRCKTPASTE != 0;
    let mut out = Vec::with_capacity(bytes.len() + 12);
    if bracketed && begin {
        out.extend_from_slice(paste_begin());
    }
    let start = out.len();
    out.extend_from_slice(bytes);
    let length = PasteFilter::new(bracketed).filter(&mut out[start..]);
    out.truncate(start + length);
    if bracketed && end {
        out.extend_from_slice(paste_end());
    }
    out
}
/// Splits a paste into chunks of at most `MAX_PASTE_BYTES`, never between CR and a following LF,
/// so filtering each chunk independently folds CRLF as filtering the whole paste does. An empty
/// paste is one empty chunk: its envelope still reaches the application.
pub fn chunks(mut bytes: &[u8]) -> Vec<&[u8]> {
    let mut chunks = Vec::new();
    loop {
        let mut length = bytes.len().min(MAX_PASTE_BYTES);
        if length < bytes.len() && bytes[length - 1] == b'\r' && bytes[length] == b'\n' {
            length -= 1;
        }
        let (chunk, rest) = bytes.split_at(length);
        chunks.push(chunk);
        if rest.is_empty() {
            return chunks;
        }
        bytes = rest;
    }
}
pub fn paste_begin() -> &'static [u8] {
    b"\x1b[200~"
}
pub fn paste_end() -> &'static [u8] {
    b"\x1b[201~"
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chunks_keep_crlf_together_and_envelopes_follow_modes() {
        let mut text = vec![b'x'; MAX_PASTE_BYTES - 1];
        text.extend_from_slice(b"\r\ny");
        let parts = chunks(&text);
        assert_eq!(
            parts.iter().map(|part| part.len()).collect::<Vec<_>>(),
            [MAX_PASTE_BYTES - 1, 3]
        );
        let joined: Vec<u8> = parts
            .iter()
            .flat_map(|part| envelope(part, false, false, 0))
            .collect();
        assert_eq!(&joined[MAX_PASTE_BYTES - 1..], b"\ry");
        assert_eq!(chunks(b""), [b""]);
        assert_eq!(envelope(b"a\nb", true, true, 0), b"a\rb");
        assert_eq!(
            envelope(b"a\x1b\nb", true, true, MODE_BRCKTPASTE),
            b"\x1b[200~a\nb\x1b[201~"
        );
        assert_eq!(envelope(b"a", false, true, MODE_BRCKTPASTE), b"a\x1b[201~");
    }
}
