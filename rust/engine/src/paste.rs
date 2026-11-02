//! Paste filtering in C. Rust keeps the buffer; C borrows it for the call.
use crate::{ffi::check, Error};
use std::ffi::CStr;
use worminal_engine_sys as sys;

pub struct PasteFilter {
    state: sys::WorminalPasteFilter,
}
impl PasteFilter {
    pub fn new(bracketed: bool) -> Self {
        Self {
            state: sys::WorminalPasteFilter {
                bracketed: i32::from(bracketed),
                previous_cr: 0,
            },
        }
    }
    pub fn filter(&mut self, bytes: &mut [u8]) -> Result<usize, Error> {
        let mut written = 0;
        check(unsafe {
            sys::worminal_paste_filter(
                &mut self.state,
                bytes.as_mut_ptr(),
                bytes.len(),
                &mut written,
            )
        })?;
        Ok(written)
    }
}
pub fn paste_begin() -> &'static [u8] {
    CStr::from_bytes_with_nul(sys::WORMINAL_PASTE_BEGIN)
        .expect("generated paste marker")
        .to_bytes()
}
pub fn paste_end() -> &'static [u8] {
    CStr::from_bytes_with_nul(sys::WORMINAL_PASTE_END)
        .expect("generated paste marker")
        .to_bytes()
}
