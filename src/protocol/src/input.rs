//! Input vocabulary of semantic Worp commands. Modifier masks use X11 (and GDK) bit values.
pub const MOD_SHIFT: u32 = 1 << 0;
pub const MOD_CONTROL: u32 = 1 << 2;
pub const MOD_ALT: u32 = 1 << 3;
pub const MOD_MOD3: u32 = 1 << 5;
pub const MOD_SUPER: u32 = 1 << 6;
/// The modifiers terminal encodings distinguish; others are ignored.
pub const MOD_KEYS: u32 = MOD_SHIFT | MOD_CONTROL | MOD_ALT | MOD_MOD3 | MOD_SUPER;

/// Zero-based xterm button codes: 0 left, 1 middle, 2 right; motion with no button held reports
/// `MOUSE_NONE`; wheel notches are `MOUSE_WHEEL_UP` and `MOUSE_WHEEL_DOWN`.
pub const MOUSE_NONE: u32 = 3;
pub const MOUSE_WHEEL_UP: u32 = 64;
pub const MOUSE_WHEEL_DOWN: u32 = 65;

/// Paste chunk ceiling: filtered text plus the bracketed envelope stays within one input operation.
pub const MAX_PASTE_BYTES: usize = 32768;
/// Wheel notches per command; peers reject larger counts.
pub const MAX_WHEEL_NOTCHES: u32 = 256;
