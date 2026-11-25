//! Presentation rules for the terminal cells received over Worp.
use worminal_protocol::cell::{ATTR_BOLD, ATTR_INVISIBLE, ATTR_REVERSE, MODE_REVERSE};

pub const FAINT_BRIGHTNESS: f64 = 0.66;

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
