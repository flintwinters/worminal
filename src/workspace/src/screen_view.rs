//! Presentation rules for the terminal cells received over Worp.
use worminal_protocol::cell::{
    ATTR_BOLD, ATTR_FAINT, ATTR_INVISIBLE, ATTR_REVERSE, ATTR_WDUMMY, ATTR_WIDE, MODE_HIDE,
    MODE_REVERSE,
};

pub const FAINT_BRIGHTNESS: f64 = 0.66;

pub fn faint(attributes: u32) -> bool {
    attributes & (ATTR_FAINT | ATTR_INVISIBLE) == ATTR_FAINT
}

pub fn cursor_visible(mode: u32, scroll: u32) -> bool {
    scroll == 0 && mode & MODE_HIDE == 0
}

/// A wide continuation has no glyph. A lead clipped by the right edge still fills its cell.
pub fn glyph(rune: u32, attributes: u32, columns_remaining: usize) -> Option<char> {
    if attributes & ATTR_WDUMMY != 0 {
        return None;
    }
    if attributes & ATTR_WIDE != 0 && columns_remaining == 1 {
        return Some(' ');
    }
    Some(match char::from_u32(rune) {
        Some(character) if character.is_control() => ' ',
        Some(character) => character,
        None => '\u{fffd}',
    })
}

pub fn colors(
    mut foreground: u32,
    mut background: u32,
    attributes: u32,
    mode: u32,
    selected: bool,
    bold_bright: bool,
) -> (u32, u32) {
    if bold_bright && attributes & ATTR_BOLD != 0 && foreground < 8 {
        foreground += 8;
    }
    if (attributes & ATTR_REVERSE != 0) ^ (mode & MODE_REVERSE != 0) ^ selected {
        std::mem::swap(&mut foreground, &mut background);
    }
    if attributes & ATTR_INVISIBLE != 0 {
        foreground = background;
    }
    (foreground, background)
}
