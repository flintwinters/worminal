//! Terminal key and pointer policy; GTK owns event delivery and Rust filters paste and the C engine owns selection semantics.
use gtk::gdk::{
    keys::{constants as key, Key},
    ModifierType as Mod,
};
use worminal_protocol::{input::MOD_KEYS, worp::commands::CommandKind};

#[derive(Debug, PartialEq, Eq)]
pub enum KeyAction {
    Send(CommandKind),
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
        // The daemon encodes the key against its live modes; the view's modes only decide that
        // the terminal, not the input method, takes it.
        let (keysym, modifiers) = (*value, state.bits() & MOD_KEYS);
        let character = value.to_unicode().filter(|&c| c != '\0');
        if worminal_engine::keys::encode(keysym, character, modifiers, mode, self.numlock).is_some()
        {
            return KeyAction::Send(CommandKind::Key {
                keysym,
                modifiers,
                character,
                numlock: self.numlock,
            });
        }
        KeyAction::Ime
    }
}

#[derive(Default)]
pub struct Mouse {
    /// The held GDK button, 0 for none.
    pub button: u32,
    pub last: (i32, i32),
}
/// Browser-style middle-click autoscroll: the pointer's distance from the click anchor sets the
/// scroll speed, with a dead zone around the anchor. Speed accelerates with distance and has no
/// fixed ceiling: once the button is released the pointer cannot report beyond the widget, so the
/// widget's height already bounds it, and a linear or capped curve made long histories slow.
pub struct Autoscroll {
    anchor: f64,
    pub y: f64,
    /// Signed fractional lines (negative is up) left over from the previous step.
    carry: f64,
}
impl Autoscroll {
    const DEAD_ZONE: f64 = 12.0;
    /// Lines per second for each line height of pointer distance beyond the dead zone, near it.
    const GAIN: f64 = 4.0;
    /// Line heights of distance at which the rate has doubled over the linear `GAIN`.
    const ACCELERATION: f64 = 4.0;
    pub fn new(y: f64) -> Self {
        Self {
            anchor: y,
            y,
            carry: 0.0,
        }
    }
    /// Whole lines to scroll after `seconds`, as `(up, lines)`.
    pub fn step(&mut self, seconds: f64, line_height: f64) -> Option<(bool, u32)> {
        let offset = self.y - self.anchor;
        let beyond = (offset.abs() - Self::DEAD_ZONE).max(0.0);
        if beyond == 0.0 {
            self.carry = 0.0;
            return None;
        }
        let distance = beyond / line_height.max(1.0);
        let rate = Self::GAIN * distance * (1.0 + distance / Self::ACCELERATION);
        self.carry += rate * seconds * offset.signum();
        let lines = self.carry.abs().floor();
        self.carry -= lines * self.carry.signum();
        (lines >= 1.0).then_some((offset < 0.0, lines as u32))
    }
}
pub fn position(px: f64, py: f64, width: i32, height: i32, columns: i32, rows: i32) -> (i32, i32) {
    (
        ((px - 2.0) as i32 / width.max(1)).clamp(0, columns - 1),
        ((py - 2.0) as i32 / height.max(1)).clamp(0, rows - 1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use worminal_protocol::cell::*;

    #[test]
    fn autoscroll_speed_follows_pointer_distance() {
        let mut scroll = Autoscroll::new(100.0);
        scroll.y = 108.0;
        assert_eq!(scroll.step(1.0, 16.0), None, "inside the dead zone");
        scroll.y = 100.0 + 12.0 + 32.0;
        assert_eq!(scroll.step(1.0, 16.0), Some((false, 12)));
        scroll.y = 100.0 - 12.0 - 16.0;
        assert_eq!(scroll.step(0.1, 16.0), None, "fractions carry");
        assert_eq!(scroll.step(0.2, 16.0), Some((true, 1)));
        scroll.y = 100.0;
        assert_eq!(
            scroll.step(1.0, 16.0),
            None,
            "returning to the anchor stops"
        );
        // Forty line heights away: far beyond the old fixed 80 lines per second.
        scroll.y = 100.0 + 12.0 + 16.0 * 40.0;
        assert_eq!(
            scroll.step(1.0, 16.0),
            Some((false, 1760)),
            "speed accelerates without a ceiling"
        );
    }

    #[test]
    fn keyboard_mapping_matches_pre_migration_behavior() {
        // Freeze the observable C-table behavior before moving its source to Rust.
        // Cover every mapped key, all supported modifier combinations and both
        // application modes with Num Lock on/off. Ordering and wildcard masks matter.
        let mut keys: Vec<_> = worminal_engine::keys::mapped_keys()
            .map(Key::from)
            .collect();
        keys.sort_by_key(|key| **key);
        keys.dedup();
        let modifiers = [
            Mod::SHIFT_MASK,
            Mod::CONTROL_MASK,
            Mod::MOD1_MASK,
            Mod::MOD4_MASK,
            Mod::MOD3_MASK,
        ];
        let mut hash = 0xcbf29ce484222325u64;
        let mut record = |byte: u8| {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        };
        for value in keys {
            for bits in 0..32 {
                let mut state = Mod::empty();
                for (i, modifier) in modifiers.iter().enumerate() {
                    if bits & (1 << i) != 0 {
                        state |= *modifier;
                    }
                }
                for mode in [
                    0,
                    MODE_APPKEYPAD,
                    MODE_APPCURSOR,
                    MODE_APPKEYPAD | MODE_APPCURSOR,
                ] {
                    for numlock in [false, true] {
                        for byte in (*value).to_le_bytes() {
                            record(byte);
                        }
                        let mask = state.bits() & MOD_KEYS;
                        let result = worminal_engine::keys::mapping(*value, mask, mode, numlock);
                        record(result.map_or(255, |bytes| bytes.len() as u8));
                        if let Some(bytes) = result {
                            for &byte in bytes {
                                record(byte);
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(
            hash, 4_334_246_905_110_447_577,
            "keyboard behavior fingerprint changed"
        );
    }
}
