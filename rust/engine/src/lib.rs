//! Exclusive terminal handles and Rust-owned copies of cells, events, and snapshots.
mod ffi;
pub use ffi::{Cell, Engine, Error, Event, Frame, TransportState};
pub use worminal_engine_sys::{
    WORMINAL_BELL, WORMINAL_CLIPBOARD, WORMINAL_CURSOR, WORMINAL_DIAGNOSTIC, WORMINAL_ICON_TITLE,
    WORMINAL_MODE, WORMINAL_PALETTE, WORMINAL_PALETTE_QUERY, WORMINAL_PALETTE_RESET,
    WORMINAL_POINTER_MOTION, WORMINAL_PRINT, WORMINAL_REPLY, WORMINAL_TITLE, WORMINAL_WRITE,
};
