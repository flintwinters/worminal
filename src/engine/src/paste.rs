//! In-place paste normalization, independent of the terminal parser and transport.
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
pub fn paste_begin() -> &'static [u8] {
    b"\x1b[200~"
}
pub fn paste_end() -> &'static [u8] {
    b"\x1b[201~"
}
