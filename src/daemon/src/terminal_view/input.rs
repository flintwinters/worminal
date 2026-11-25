//! Forward the outer terminal's VT keystrokes, reserving a single command prefix.
const PREFIX: u8 = 0x1d;
const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";

pub(super) enum Action {
    Bytes(Vec<u8>),
    Detach,
    New,
    Next,
    Previous,
    Scroll(bool),
}

#[derive(Default)]
pub(super) struct Input {
    prefix: bool,
    paste: bool,
    matched: usize,
}

impl Input {
    pub fn feed(&mut self, input: &[u8]) -> Vec<Action> {
        let mut actions = Vec::new();
        let mut bytes = Vec::new();
        for &byte in input {
            if self.prefix {
                self.prefix = false;
                let action = match byte {
                    b'q' => Action::Detach,
                    b't' => Action::New,
                    b'n' => Action::Next,
                    b'p' => Action::Previous,
                    b'k' => Action::Scroll(true),
                    b'j' => Action::Scroll(false),
                    _ => {
                        bytes.push(PREFIX);
                        if byte != PREFIX {
                            bytes.push(byte);
                        }
                        continue;
                    }
                };
                if !bytes.is_empty() {
                    actions.push(Action::Bytes(std::mem::take(&mut bytes)));
                }
                actions.push(action);
                continue;
            }
            let marker = if self.paste { PASTE_END } else { PASTE_START };
            if byte == marker[self.matched] {
                self.matched += 1;
                if self.matched == marker.len() {
                    self.paste = !self.paste;
                    self.matched = 0;
                }
            } else {
                self.matched = usize::from(byte == marker[0]);
            }
            if byte == PREFIX && !self.paste {
                self.prefix = true;
            } else {
                bytes.push(byte);
            }
        }
        if !bytes.is_empty() {
            actions.push(Action::Bytes(bytes));
        }
        actions
    }
}
