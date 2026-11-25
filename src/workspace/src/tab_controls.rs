//! Tab actions shared by graphical and terminal presentations.
use worminal_protocol::input::{MOD_ALT, MOD_CONTROL, MOD_SHIFT};

#[derive(Clone, Copy)]
pub enum Action {
    New,
    Close,
    Bookmark,
    Cycle { previous: bool, tinted_only: bool },
}

pub fn shortcut(key: char, modifiers: u32) -> Option<Action> {
    let shift = modifiers & MOD_SHIFT != 0;
    match (key.to_ascii_lowercase(), modifiers & !MOD_SHIFT) {
        ('t', MOD_CONTROL) if !shift => Some(Action::New),
        ('w', MOD_CONTROL) if !shift => Some(Action::Close),
        ('d', MOD_CONTROL) if shift => Some(Action::Bookmark),
        ('\t', MOD_CONTROL | MOD_ALT) => Some(Action::Cycle {
            previous: shift,
            tinted_only: modifiers & MOD_ALT != 0,
        }),
        _ => None,
    }
}

pub fn wheel_cycles(modifiers: u32, over_tab_bar: bool) -> bool {
    over_tab_bar || modifiers & MOD_CONTROL != 0
}

pub fn cycle(
    current: usize,
    count: usize,
    previous: bool,
    mut eligible: impl FnMut(usize) -> bool,
) -> Option<usize> {
    (1..count)
        .map(|step| (current + if previous { count - step } else { step }) % count)
        .find(|&index| eligible(index))
}

/// The daemon expects the destination index after removing the moved tab.
pub fn move_position(from: usize, to: usize, after: bool) -> u32 {
    (to + usize::from(after) - usize::from(from < to)) as u32
}
