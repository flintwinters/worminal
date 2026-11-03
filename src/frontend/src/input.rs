//! Terminal key and pointer policy; GTK owns event delivery and the C engine owns paste/selection semantics.
use gtk::gdk::{
    self,
    keys::{constants as key, Key},
    ModifierType as Mod,
};
use worminal_engine_sys::*;

struct KeyEntry {
    key: Key,
    mask: u32,
    bytes: &'static [u8],
    appkey: i32,
    appcursor: i32,
}
include!(concat!(env!("OUT_DIR"), "/keymap.rs"));

#[derive(Debug, PartialEq, Eq)]
pub enum KeyAction {
    Bytes(Vec<u8>),
    PastePrimary,
    Scroll { up: bool, rows: u32 },
    Control(u32),
    PrintSelection,
    Consumed,
    Ime,
}
pub struct Keyboard {
    numlock: bool,
}
impl Default for Keyboard {
    fn default() -> Self {
        Self { numlock: true }
    }
}
impl Keyboard {
    pub fn press(&mut self, value: Key, state: Mod, mode: u32, rows: u32) -> KeyAction {
        if value == key::Num_Lock && state.contains(Mod::CONTROL_MASK | Mod::SHIFT_MASK) {
            self.numlock = !self.numlock;
            return KeyAction::Consumed;
        }
        if value == key::Insert && state.contains(Mod::SHIFT_MASK) {
            return KeyAction::PastePrimary;
        }
        if value == key::Print || value == key::Break {
            return if value == key::Print && !state.intersects(Mod::CONTROL_MASK | Mod::SHIFT_MASK)
            {
                KeyAction::PrintSelection
            } else {
                KeyAction::Control(if value == key::Break {
                    2
                } else if state.contains(Mod::CONTROL_MASK) {
                    0
                } else {
                    1
                })
            };
        }
        if state.contains(Mod::SHIFT_MASK) && (value == key::Page_Up || value == key::Page_Down) {
            return KeyAction::Scroll {
                up: value == key::Page_Up,
                rows: rows.saturating_sub(1).max(1),
            };
        }
        if let Some(bytes) = self.mapping(value, state, mode) {
            return KeyAction::Bytes(bytes.to_vec());
        }
        if state.contains(Mod::CONTROL_MASK) {
            // Encode ASCII Control before the input method, including NUL and numeric aliases.
            // Locale strings can lose embedded NUL; punctuation pairs share five-bit encoding.
            if let Some(c) = value.to_unicode().and_then(|c| match c {
                '@'..='~' | ' ' => Some(c as u8 & 31),
                '2' => Some(0),
                '3'..='7' => Some(c as u8 - b'3' + 27),
                '8' | '?' => Some(127),
                '/' => Some(31),
                _ => None,
            }) {
                let mut bytes = Vec::new();
                if state.contains(Mod::MOD1_MASK) {
                    bytes.push(0x1b);
                }
                bytes.push(c);
                return KeyAction::Bytes(bytes);
            }
        }
        if state.contains(Mod::MOD1_MASK) {
            if let Some(c) = value.to_unicode().filter(|c| (' '..'\u{7f}').contains(c)) {
                let mut bytes = Vec::new();
                let c = if mode & MODE_8BIT != 0 {
                    char::from_u32(u32::from(c) | 0x80).unwrap()
                } else {
                    bytes.push(0x1b);
                    c
                };
                bytes.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
                return KeyAction::Bytes(bytes);
            }
        }
        KeyAction::Ime
    }
    fn mapping(&self, value: Key, state: Mod, mode: u32) -> Option<&'static [u8]> {
        let mask = (state
            & (Mod::SHIFT_MASK
                | Mod::CONTROL_MASK
                | Mod::MOD1_MASK
                | Mod::MOD4_MASK
                | Mod::MOD3_MASK))
            .bits();
        KEYS.iter()
            .find(|entry| {
                entry.key == value
                    && (entry.mask == u32::MAX || entry.mask == mask)
                    && (if mode & MODE_APPKEYPAD != 0 {
                        entry.appkey >= 0
                    } else {
                        entry.appkey <= 0
                    })
                    && !(self.numlock && entry.appkey == 2)
                    && (if mode & MODE_APPCURSOR != 0 {
                        entry.appcursor >= 0
                    } else {
                        entry.appcursor <= 0
                    })
            })
            .map(|entry| entry.bytes)
    }
}

#[derive(Default)]
pub struct Mouse {
    pub button: u32,
    pub last: (i32, i32),
}
impl Mouse {
    pub fn report(
        &self,
        mode: u32,
        live: bool,
        button: u32,
        x: i32,
        y: i32,
        state: Mod,
        release: bool,
        motion: bool,
    ) -> Option<Vec<u8>> {
        if motion && !live || mode & MODE_MOUSEX10 != 0 && (release || motion) {
            return None;
        }
        if motion
            && mode & MODE_MOUSEMANY == 0
            && !(self.button != 0 && mode & MODE_MOUSEMOTION != 0)
        {
            return None;
        }
        let mut code = button.checked_add(if motion { 32 } else { 0 })?;
        if mode & MODE_MOUSEX10 == 0 {
            if state.contains(Mod::SHIFT_MASK) {
                code = code.checked_add(4)?;
            }
            if state.contains(Mod::MOD1_MASK) {
                code = code.checked_add(8)?;
            }
            if state.contains(Mod::CONTROL_MASK) {
                code = code.checked_add(16)?;
            }
        }
        if mode & MODE_MOUSESGR != 0 {
            Some(
                format!(
                    "\x1b[<{code};{};{}{}",
                    x + 1,
                    y + 1,
                    if release { 'm' } else { 'M' }
                )
                .into_bytes(),
            )
        } else {
            if !(0..223).contains(&x) || !(0..223).contains(&y) || code > 223 {
                return None;
            }
            Some(vec![
                0x1b,
                b'[',
                b'M',
                if release { 35 } else { 32 + code as u8 },
                33 + x as u8,
                33 + y as u8,
            ])
        }
    }
}
pub fn position(px: f64, py: f64, width: i32, height: i32, columns: i32, rows: i32) -> (i32, i32) {
    (
        ((px - 2.0) as i32 / width.max(1)).clamp(0, columns - 1),
        ((py - 2.0) as i32 / height.max(1)).clamp(0, rows - 1),
    )
}
