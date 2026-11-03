//! Exclusive terminal handles and Rust-owned copies of cells, events, and snapshots.
mod ffi;
mod paste;
mod selection;
pub use ffi::{Cell, Engine, Error, Event, Frame, TransportState};
pub use paste::{paste_begin, paste_end, PasteFilter};
pub use selection::{Selection, SelectionRange};
pub use worminal_engine_sys::WORMINAL_PENDING_BYTES;
pub use worminal_engine_sys::{MODE_FOCUS, MODE_REVERSE};
pub use worminal_engine_sys::{
    WORMINAL_BELL, WORMINAL_CLIPBOARD, WORMINAL_CURSOR, WORMINAL_DIAGNOSTIC, WORMINAL_ICON_TITLE,
    WORMINAL_MODE, WORMINAL_PALETTE, WORMINAL_PALETTE_QUERY, WORMINAL_PALETTE_RESET,
    WORMINAL_POINTER_MOTION, WORMINAL_PRINT, WORMINAL_PRINT_SCREEN, WORMINAL_REPLY, WORMINAL_TITLE,
    WORMINAL_TOGGLE_PRINTER, WORMINAL_WRITE,
};
pub use worminal_engine_sys::{
    WORMINAL_DEFAULT_BACKGROUND, WORMINAL_DEFAULT_CURSOR, WORMINAL_DEFAULT_FOREGROUND,
};
