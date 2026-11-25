//! Translate outer terminal events and keep ordinary VT input and bracketed paste byte-exact.
use worminal_protocol::input::{MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_SUPER};
use worminal_protocol::worp::commands::CommandKind;
use worminal_workspace::tab_controls::{self, Action as TabAction};

const PREFIX: u8 = 0x1d;
const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";

pub(super) enum Action {
    Bytes(Vec<u8>),
    Detach,
    Prefix,
    Tab(TabAction),
    Scroll(bool),
    Pointer(Pointer),
    Key(CommandKind),
}

pub(super) struct Pointer {
    pub code: u32,
    pub column: u32,
    pub row: u32,
    pub release: bool,
}

#[derive(Default)]
pub(super) struct Input {
    prefix: bool,
    paste: Option<usize>,
    escape: Vec<u8>,
}

impl Input {
    pub fn pending(&self) -> bool {
        !self.escape.is_empty()
    }

    pub fn flush_escape(&mut self) -> Action {
        let mut bytes = std::mem::take(&mut self.escape);
        if std::mem::take(&mut self.prefix) {
            bytes.insert(0, PREFIX);
        }
        Action::Bytes(bytes)
    }

    pub fn feed(&mut self, input: &[u8]) -> Vec<Action> {
        let mut actions = Vec::new();
        let mut bytes = Vec::new();
        for &byte in input {
            if let Some(matched) = self.paste {
                bytes.push(byte);
                let next = if byte == PASTE_END[matched] {
                    matched + 1
                } else {
                    usize::from(byte == PASTE_END[0])
                };
                self.paste = (next != PASTE_END.len()).then_some(next);
                continue;
            }

            if self.pending() {
                if byte != 0x1b {
                    self.escape.push(byte);
                    if self.escape == PASTE_START {
                        self.paste = Some(0);
                    } else if self.escape == b"\x1b["
                        || (self.escape.starts_with(b"\x1b[")
                            && (0x20..=0x3f).contains(&byte)
                            && self.escape.len() < 64)
                    {
                        continue;
                    } else if let Some(action) = csi(&self.escape) {
                        if matches!(action, Action::Prefix) {
                            self.push_prefix(&mut bytes);
                        } else {
                            self.literal_prefix(&mut bytes);
                            append(&mut actions, &mut bytes, action);
                        }
                        self.escape.clear();
                        continue;
                    }
                    self.literal_prefix(&mut bytes);
                    bytes.append(&mut self.escape);
                    continue;
                }
                self.literal_prefix(&mut bytes);
                bytes.append(&mut self.escape);
            }

            if byte == 0x1b {
                self.escape.push(byte);
                continue;
            }
            if byte == PREFIX {
                self.push_prefix(&mut bytes);
                continue;
            }
            if self.prefix {
                if let Some(action) = prefix(byte) {
                    self.prefix = false;
                    append(&mut actions, &mut bytes, action);
                    continue;
                }
                self.literal_prefix(&mut bytes);
            }
            match byte {
                1..=26 => {
                    let action = tab_controls::shortcut(char::from(byte | 0x60), MOD_CONTROL);
                    if let Some(action) = action {
                        append(&mut actions, &mut bytes, Action::Tab(action));
                    } else {
                        bytes.push(byte);
                    }
                }
                _ => bytes.push(byte),
            }
        }
        if !bytes.is_empty() {
            actions.push(Action::Bytes(bytes));
        }
        actions
    }
    fn literal_prefix(&mut self, bytes: &mut Vec<u8>) {
        if std::mem::take(&mut self.prefix) {
            bytes.push(PREFIX);
        }
    }

    fn push_prefix(&mut self, bytes: &mut Vec<u8>) {
        if self.prefix {
            self.literal_prefix(bytes);
        } else {
            self.prefix = true;
        }
    }
}

fn append(actions: &mut Vec<Action>, bytes: &mut Vec<u8>, action: Action) {
    if !bytes.is_empty() {
        actions.push(Action::Bytes(std::mem::take(bytes)));
    }
    actions.push(action);
}

fn prefix(byte: u8) -> Option<Action> {
    Some(match byte {
        b'q' => Action::Detach,
        b't' => Action::Tab(TabAction::New),
        b'n' | b'p' => Action::Tab(TabAction::Cycle {
            previous: byte == b'p',
            tinted_only: false,
        }),
        b'k' | b'j' => Action::Scroll(byte == b'k'),
        _ => return None,
    })
}

fn csi(bytes: &[u8]) -> Option<Action> {
    let (last, body) = bytes.strip_prefix(b"\x1b[")?.split_last()?;
    let body = std::str::from_utf8(body).ok()?;
    let mut params = body
        .trim_start_matches('<')
        .split(';')
        .map(str::parse::<u32>);
    let first = params.next()?.ok()?;
    let second = params.next().transpose().ok()?;
    let third = params.next().transpose().ok()?;
    if params.next().is_some() {
        return None;
    }
    match *last {
        b'M' | b'm' if body.starts_with('<') => Some(Action::Pointer(Pointer {
            code: first,
            column: second?.checked_sub(1)?,
            row: third?.checked_sub(1)?,
            release: *last == b'm',
        })),
        b'u' if third.is_none() => key(first, second.unwrap_or(1)),
        b'~' if first == 27 => key(third?, second?),
        b'Z' if first == 1 && second.is_some() => key(9, second?),
        _ => None,
    }
}

fn key(code: u32, parameter: u32) -> Option<Action> {
    let flags = parameter.checked_sub(1)?;
    let modifiers = ((flags & 1) * MOD_SHIFT)
        | (((flags >> 1) & 1) * MOD_ALT)
        | (((flags >> 2) & 1) * MOD_CONTROL)
        | (((flags >> 3) & 1) * MOD_SUPER);
    // Caps Lock and Num Lock do not change which workspace shortcut was pressed.
    if flags & !(0x0f | 64 | 128) == 0 {
        if let Some(action) =
            char::from_u32(code).and_then(|key| tab_controls::shortcut(key, modifiers))
        {
            return Some(Action::Tab(action));
        }
    }
    if code == u32::from(PREFIX) || (code == u32::from(']') && modifiers == MOD_CONTROL) {
        // Kitty reports the physical key and modifier instead of the legacy control byte.
        return Some(Action::Prefix);
    }
    let character = char::from_u32(code)?;
    let keysym = match code {
        9 => 0xff09,
        13 => 0xff0d,
        27 => 0xff1b,
        127 => 0xff08,
        32..=126 | 160..=255 => code,
        _ if !character.is_control() && !(57344..=63743).contains(&code) => 0x0100_0000 | code,
        _ => return None,
    };
    Some(Action::Key(CommandKind::Key {
        keysym,
        character: (!character.is_control()).then_some(character),
        modifiers,
        numlock: true,
    }))
}
