//! Screen vocabulary of the Worp frame stream: what a frontend needs to draw rows and interpret
//! modes without linking the terminal engine. Each wire cell is `WIRE_CELL_BYTES` of big-endian
//! `u32` fields in this order: rune (Unicode scalar, 0 for blank), attributes (`ATTR_*`), foreground
//! and background (`COLOR_*` encoding). The engine pins its own values to these at compile time.

// Cell attributes.
pub const ATTR_BOLD: u32 = 1 << 0;
pub const ATTR_FAINT: u32 = 1 << 1;
pub const ATTR_ITALIC: u32 = 1 << 2;
pub const ATTR_UNDERLINE: u32 = 1 << 3;
pub const ATTR_BLINK: u32 = 1 << 4;
pub const ATTR_REVERSE: u32 = 1 << 5;
pub const ATTR_INVISIBLE: u32 = 1 << 6;
pub const ATTR_STRUCK: u32 = 1 << 7;
/// The row continues on the next row (soft wrap) after this, its last cell.
pub const ATTR_WRAP: u32 = 1 << 8;
/// The first column of a double-width character.
pub const ATTR_WIDE: u32 = 1 << 9;
/// The covered second column of a double-width character; draw nothing.
pub const ATTR_WDUMMY: u32 = 1 << 10;

// Colors: below `COLOR_COUNT` a palette index (0-255 xterm, then the defaults below), otherwise
// `COLOR_TRUECOLOR | 0xRRGGBB`.
pub const COLOR_TRUECOLOR: u32 = 1 << 24;
pub const COLOR_COUNT: u32 = 270;
pub const COLOR_DEFAULT_CURSOR: u32 = 256;
pub const COLOR_DEFAULT_FOREGROUND: u32 = 258;
pub const COLOR_DEFAULT_BACKGROUND: u32 = 259;

// Terminal modes, reported by catalog entries.
pub const MODE_VISIBLE: u32 = 1 << 0;
pub const MODE_FOCUSED: u32 = 1 << 1;
pub const MODE_APPKEYPAD: u32 = 1 << 2;
pub const MODE_MOUSEBTN: u32 = 1 << 3;
pub const MODE_MOUSEMOTION: u32 = 1 << 4;
pub const MODE_REVERSE: u32 = 1 << 5;
pub const MODE_KBDLOCK: u32 = 1 << 6;
pub const MODE_HIDE: u32 = 1 << 7;
pub const MODE_APPCURSOR: u32 = 1 << 8;
pub const MODE_MOUSESGR: u32 = 1 << 9;
pub const MODE_8BIT: u32 = 1 << 10;
pub const MODE_BLINK: u32 = 1 << 11;
pub const MODE_FBLINK: u32 = 1 << 12;
pub const MODE_FOCUS: u32 = 1 << 13;
pub const MODE_MOUSEX10: u32 = 1 << 14;
pub const MODE_MOUSEMANY: u32 = 1 << 15;
pub const MODE_BRCKTPASTE: u32 = 1 << 16;
pub const MODE_NUMLOCK: u32 = 1 << 17;
pub const MODE_ALTSCROLL: u32 = 1 << 18;
/// OSC 9;4 reports an active task.
pub const MODE_BUSY: u32 = 1 << 19;
/// Daemon-owned catalog mode: a live shell job outside the foreground process group.
pub const MODE_BACKGROUND_JOB: u32 = 1 << 20;
pub const MODE_MOUSE: u32 = MODE_MOUSEBTN | MODE_MOUSEMOTION | MODE_MOUSEX10 | MODE_MOUSEMANY;
