//! Compile-time pin of `engine.h` to the protocol's screen vocabulary: frames copy native cells and
//! modes onto the wire unchanged, so any divergence must fail the build rather than miscolor peers.
use worminal_engine_sys as sys;
use worminal_protocol::{cell, WIRE_CELL_BYTES};

macro_rules! pin {
    ($($native:ident = $wire:ident),* $(,)?) => {
        $(const _: () = assert!(sys::$native == cell::$wire, stringify!($native));)*
    };
}

pin! {
    ATTR_BOLD = ATTR_BOLD, ATTR_FAINT = ATTR_FAINT, ATTR_ITALIC = ATTR_ITALIC,
    ATTR_UNDERLINE = ATTR_UNDERLINE, ATTR_BLINK = ATTR_BLINK, ATTR_REVERSE = ATTR_REVERSE,
    ATTR_INVISIBLE = ATTR_INVISIBLE, ATTR_STRUCK = ATTR_STRUCK, ATTR_WRAP = ATTR_WRAP,
    ATTR_WIDE = ATTR_WIDE, ATTR_WDUMMY = ATTR_WDUMMY,
    WORMINAL_TRUECOLOR_FLAG = COLOR_TRUECOLOR, WORMINAL_COLOR_COUNT = COLOR_COUNT,
    WORMINAL_DEFAULT_CURSOR = COLOR_DEFAULT_CURSOR,
    WORMINAL_DEFAULT_FOREGROUND = COLOR_DEFAULT_FOREGROUND,
    WORMINAL_DEFAULT_BACKGROUND = COLOR_DEFAULT_BACKGROUND,
    MODE_VISIBLE = MODE_VISIBLE, MODE_FOCUSED = MODE_FOCUSED, MODE_APPKEYPAD = MODE_APPKEYPAD,
    MODE_MOUSEBTN = MODE_MOUSEBTN, MODE_MOUSEMOTION = MODE_MOUSEMOTION, MODE_REVERSE = MODE_REVERSE,
    MODE_KBDLOCK = MODE_KBDLOCK, MODE_HIDE = MODE_HIDE, MODE_APPCURSOR = MODE_APPCURSOR,
    MODE_MOUSESGR = MODE_MOUSESGR, MODE_8BIT = MODE_8BIT, MODE_BLINK = MODE_BLINK,
    MODE_FBLINK = MODE_FBLINK, MODE_FOCUS = MODE_FOCUS, MODE_MOUSEX10 = MODE_MOUSEX10,
    MODE_MOUSEMANY = MODE_MOUSEMANY, MODE_BRCKTPASTE = MODE_BRCKTPASTE, MODE_NUMLOCK = MODE_NUMLOCK,
    MODE_ALTSCROLL = MODE_ALTSCROLL, MODE_BUSY = MODE_BUSY, MODE_MOUSE = MODE_MOUSE,
}

// The daemon's catalog bit must stay clear of every engine mode, and the native cell must hold
// exactly the four wire fields.
const _: () = assert!(cell::MODE_BACKGROUND_JOB > sys::MODE_BUSY);
const _: () = assert!(std::mem::size_of::<sys::WorminalCell>() == WIRE_CELL_BYTES as usize);
