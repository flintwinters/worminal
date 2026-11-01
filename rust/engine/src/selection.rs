//! Audited selection FFI: C borrows Rust cells for one call and retains only coordinates.
use crate::ffi::check;
use crate::{Cell, Error, Frame};
use std::ptr::{self, NonNull};
use worminal_engine_sys as sys;

pub type SelectionRange = sys::WorminalSelectionRange;

pub struct Selection {
    handle: NonNull<sys::WorminalSelection>,
}

impl Selection {
    pub fn new(frame: &Frame) -> Result<Self, Error> {
        let mut handle = ptr::null_mut();
        check(unsafe { sys::worminal_selection_create(&mut handle) })?;
        let mut selection = Self {
            handle: NonNull::new(handle).expect("successful selection creation returned null"),
        };
        selection.configure(frame)?;
        Ok(selection)
    }

    pub fn configure(&mut self, frame: &Frame) -> Result<(), Error> {
        check(unsafe { sys::worminal_selection_configure(self.handle.as_ptr(), frame) })
    }

    pub fn clear(&mut self) -> Result<(), Error> {
        check(unsafe { sys::worminal_selection_clear(self.handle.as_ptr()) })
    }

    pub fn select(
        &mut self,
        cells: &[Cell],
        column: i32,
        row: i32,
        start: bool,
        snap: i32,
        rectangular: bool,
        done: bool,
    ) -> Result<(), Error> {
        check(unsafe {
            sys::worminal_selection_select(
                self.handle.as_ptr(),
                cells.as_ptr(),
                cells.len(),
                column,
                row,
                i32::from(start),
                snap,
                if rectangular {
                    sys::SEL_RECTANGULAR as i32
                } else {
                    sys::SEL_REGULAR as i32
                },
                i32::from(done),
            )
        })
    }

    pub fn text(&mut self, cells: &[Cell]) -> Result<Vec<u8>, Error> {
        let mut required = 0;
        let status = unsafe {
            sys::worminal_selection_text(
                self.handle.as_ptr(),
                cells.as_ptr(),
                cells.len(),
                ptr::null_mut(),
                0,
                &mut required,
            )
        };
        if status != sys::WORMINAL_CAPACITY as i32 {
            check(status)?;
        }
        let mut output = vec![0; required];
        check(unsafe {
            sys::worminal_selection_text(
                self.handle.as_ptr(),
                cells.as_ptr(),
                cells.len(),
                output.as_mut_ptr(),
                output.len(),
                &mut required,
            )
        })?;
        output.truncate(required);
        Ok(output)
    }

    pub fn ranges(&mut self, output: &mut Vec<SelectionRange>) -> Result<(), Error> {
        let mut required = 0;
        let status = unsafe {
            sys::worminal_selection_ranges(self.handle.as_ptr(), ptr::null_mut(), 0, &mut required)
        };
        if status != sys::WORMINAL_CAPACITY as i32 {
            check(status)?;
        }
        output.resize(required, SelectionRange::default());
        check(unsafe {
            sys::worminal_selection_ranges(
                self.handle.as_ptr(),
                output.as_mut_ptr(),
                output.len(),
                &mut required,
            )
        })?;
        output.truncate(required);
        Ok(())
    }
}

impl Drop for Selection {
    fn drop(&mut self) {
        unsafe { sys::worminal_selection_destroy(self.handle.as_ptr()) }
    }
}
