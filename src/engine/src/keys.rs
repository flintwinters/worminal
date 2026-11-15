//! Shared terminal key encoding; independent of GTK and evaluated against the target terminal mode.
use worminal_engine_sys::{MODE_APPCURSOR, MODE_APPKEYPAD};
#[allow(non_upper_case_globals)]
mod key {
    pub const BackSpace: u32 = 0xff08;
    pub const Delete: u32 = 0xffff;
    pub const Down: u32 = 0xff54;
    pub const End: u32 = 0xff57;
    pub const F1: u32 = 0xffbe;
    pub const F10: u32 = 0xffc7;
    pub const F11: u32 = 0xffc8;
    pub const F12: u32 = 0xffc9;
    pub const F13: u32 = 0xffca;
    pub const F14: u32 = 0xffcb;
    pub const F15: u32 = 0xffcc;
    pub const F16: u32 = 0xffcd;
    pub const F17: u32 = 0xffce;
    pub const F18: u32 = 0xffcf;
    pub const F19: u32 = 0xffd0;
    pub const F2: u32 = 0xffbf;
    pub const F20: u32 = 0xffd1;
    pub const F21: u32 = 0xffd2;
    pub const F22: u32 = 0xffd3;
    pub const F23: u32 = 0xffd4;
    pub const F24: u32 = 0xffd5;
    pub const F25: u32 = 0xffd6;
    pub const F26: u32 = 0xffd7;
    pub const F27: u32 = 0xffd8;
    pub const F28: u32 = 0xffd9;
    pub const F29: u32 = 0xffda;
    pub const F3: u32 = 0xffc0;
    pub const F30: u32 = 0xffdb;
    pub const F31: u32 = 0xffdc;
    pub const F32: u32 = 0xffdd;
    pub const F33: u32 = 0xffde;
    pub const F34: u32 = 0xffdf;
    pub const F35: u32 = 0xffe0;
    pub const F4: u32 = 0xffc1;
    pub const F5: u32 = 0xffc2;
    pub const F6: u32 = 0xffc3;
    pub const F7: u32 = 0xffc4;
    pub const F8: u32 = 0xffc5;
    pub const F9: u32 = 0xffc6;
    pub const Home: u32 = 0xff50;
    pub const ISO_Left_Tab: u32 = 0xfe20;
    pub const Insert: u32 = 0xff63;
    pub const KP_0: u32 = 0xffb0;
    pub const KP_1: u32 = 0xffb1;
    pub const KP_2: u32 = 0xffb2;
    pub const KP_3: u32 = 0xffb3;
    pub const KP_4: u32 = 0xffb4;
    pub const KP_5: u32 = 0xffb5;
    pub const KP_6: u32 = 0xffb6;
    pub const KP_7: u32 = 0xffb7;
    pub const KP_8: u32 = 0xffb8;
    pub const KP_9: u32 = 0xffb9;
    pub const KP_Add: u32 = 0xffab;
    pub const KP_Begin: u32 = 0xff9d;
    pub const KP_Decimal: u32 = 0xffae;
    pub const KP_Delete: u32 = 0xff9f;
    pub const KP_Divide: u32 = 0xffaf;
    pub const KP_Down: u32 = 0xff99;
    pub const KP_End: u32 = 0xff9c;
    pub const KP_Enter: u32 = 0xff8d;
    pub const KP_Home: u32 = 0xff95;
    pub const KP_Insert: u32 = 0xff9e;
    pub const KP_Left: u32 = 0xff96;
    pub const KP_Multiply: u32 = 0xffaa;
    pub const KP_Next: u32 = 0xff9b;
    pub const KP_Prior: u32 = 0xff9a;
    pub const KP_Right: u32 = 0xff98;
    pub const KP_Subtract: u32 = 0xffad;
    pub const KP_Up: u32 = 0xff97;
    pub const Left: u32 = 0xff51;
    pub const Next: u32 = 0xff56;
    pub const Prior: u32 = 0xff55;
    pub const Return: u32 = 0xff0d;
    pub const Right: u32 = 0xff53;
    pub const Up: u32 = 0xff52;
}
struct KeyEntry {
    key: u32,
    mask: u32,
    bytes: &'static [u8],
    appkey: i32,
    appcursor: i32,
}
#[path = "keymap.rs"]
mod keymap;

pub fn mapped_keys() -> impl Iterator<Item = u32> {
    keymap::KEYS.iter().map(|entry| entry.key)
}

pub fn mapping(value: u32, mask: u32, mode: u32, numlock: bool) -> Option<&'static [u8]> {
    keymap::KEYS
        .iter()
        .find(|entry| {
            entry.key == value
                && (entry.mask == u32::MAX || entry.mask == mask)
                && (if mode & MODE_APPKEYPAD != 0 {
                    entry.appkey >= 0
                } else {
                    entry.appkey <= 0
                })
                && !(numlock && entry.appkey == 2)
                && (if mode & MODE_APPCURSOR != 0 {
                    entry.appcursor >= 0
                } else {
                    entry.appcursor <= 0
                })
        })
        .map(|entry| entry.bytes)
}

/// CLI names use Ctrl-/Alt-/Shift-/Super- prefixes and the same ordered table as GUI input.
pub fn named(name: &str, mode: u32) -> Result<Vec<u8>, &'static str> {
    let name = name.to_ascii_lowercase();
    let mut parts = name.split('-');
    let mut value = parts.next().unwrap_or_default();
    let mut mask = 0;
    for next in parts {
        let flag = match value {
            "shift" => 1,
            "ctrl" => 4,
            "alt" => 8,
            "super" => 64,
            _ => return Err("Unknown key modifier; use Ctrl-, Alt-, Shift-, Super-."),
        };
        if mask & flag != 0 {
            return Err("Repeated key modifier.");
        }
        mask |= flag;
        value = next;
    }
    let keysym = match value {
        "enter" | "return" => key::Return,
        "tab" if mask & 1 != 0 => key::ISO_Left_Tab,
        "backspace" => key::BackSpace,
        "up" => key::Up,
        "down" => key::Down,
        "left" => key::Left,
        "right" => key::Right,
        "home" => key::Home,
        "end" => key::End,
        "insert" => key::Insert,
        "delete" => key::Delete,
        "pageup" => key::Prior,
        "pagedown" => key::Next,
        "tab" if mask == 0 => return Ok(vec![b'\t']),
        "escape" | "esc" if mask == 0 => return Ok(vec![27]),
        value if value.starts_with('f') => {
            let n = value[1..]
                .parse::<u32>()
                .map_err(|_| "Unknown function key.")?;
            if !(1..=35).contains(&n) {
                return Err("Function keys are F1..F35.");
            }
            key::F1 + n - 1
        }
        value if value.len() == 1 && value.as_bytes()[0].is_ascii_alphabetic() => {
            let byte = value.as_bytes()[0];
            if mask & 4 != 0 && mask & !(1 | 4 | 8) == 0 {
                let mut bytes = if mask & 8 != 0 { vec![27] } else { Vec::new() };
                bytes.push(byte - b'a' + 1);
                return Ok(bytes);
            }
            if mask == 8 || mask == 9 {
                return Ok(vec![
                    27,
                    if mask & 1 != 0 {
                        byte.to_ascii_uppercase()
                    } else {
                        byte
                    },
                ]);
            }
            return Err("Letters require Ctrl- or Alt-; use type for text.");
        }
        _ => return Err("Unknown key; run worminald api help keys."),
    };
    mapping(keysym, mask, mode, true)
        .map(<[u8]>::to_vec)
        .ok_or("Unsupported key/modifier combination; run worminald api help keys.")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn named_keys_use_target_modes_and_ordered_modifiers() {
        assert_eq!(named("Up", 0).unwrap(), b"\x1b[A");
        assert_eq!(named("Up", MODE_APPCURSOR).unwrap(), b"\x1bOA");
        assert_eq!(named("Ctrl-Shift-Left", 0).unwrap(), b"\x1b[1;6D");
        assert_eq!(named("F12", 0).unwrap(), b"\x1b[24~");
        assert_eq!(named("Alt-Ctrl-C", 0).unwrap(), b"\x1b\x03");
        for name in ["F0", "F36", "Ctrl-Ctrl-A", "unknown", "Ctrl-unknown"] {
            assert!(named(name, 0).is_err());
        }
    }
}
