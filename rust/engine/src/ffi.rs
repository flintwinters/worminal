//! Audited FFI boundary. Handles never escape; every C buffer points into a live Rust allocation.
use std::ptr::NonNull;
use worminal_engine_sys as sys;

pub type Cell = sys::WorminalCell;
pub type Frame = sys::WorminalFrame;
pub type TransportState = sys::WorminalTransportState;

#[derive(Debug, PartialEq, Eq)]
pub struct Error(pub i32);

fn check(status: i32) -> Result<(), Error> {
    if status == sys::WORMINAL_OK as i32 {
        Ok(())
    } else {
        Err(Error(status))
    }
}

#[derive(Debug)]
pub struct Event {
    pub kind: u32,
    pub argument: i32,
    pub flags: u32,
    pub data: Vec<u8>,
}

/// A single owner; NonNull also prevents implicit cross-thread sharing of the C handle.
pub struct Engine {
    handle: NonNull<sys::WorminalEngine>,
}

impl Engine {
    pub fn new(columns: i32, rows: i32) -> Result<Self, Error> {
        let mut handle = std::ptr::null_mut();
        // C publishes a handle only after initialization; Drop is its only release path.
        check(unsafe { sys::worminal_engine_create(columns, rows, &mut handle) })?;
        Ok(Self {
            handle: NonNull::new(handle).expect("successful engine creation returned null"),
        })
    }

    /// Returns consumed bytes and ordered effects. Apply effects before feeding the next bytes.
    /// Keep an incomplete UTF-8 suffix with the application's pending PTY output.
    pub fn feed(&mut self, bytes: &[u8], now: i64) -> Result<(usize, Vec<Event>), Error> {
        let mut consumed = 0;
        check(unsafe {
            sys::worminal_engine_feed(
                self.handle.as_ptr(),
                bytes.as_ptr(),
                bytes.len(),
                now,
                &mut consumed,
            )
        })?;
        Ok((consumed, self.events()?))
    }

    pub fn resize(&mut self, columns: i32, rows: i32) -> Result<(), Error> {
        check(unsafe { sys::worminal_engine_resize(self.handle.as_ptr(), columns, rows) })
    }

    pub fn input(&mut self, bytes: &[u8], echo: bool, now: i64) -> Result<Vec<Event>, Error> {
        check(unsafe {
            sys::worminal_engine_input(
                self.handle.as_ptr(),
                bytes.as_ptr(),
                bytes.len(),
                i32::from(echo),
                now,
            )
        })?;
        self.events()
    }

    pub fn scroll(&mut self, up: bool, rows: i32) -> Result<(), Error> {
        check(unsafe { sys::worminal_engine_scroll(self.handle.as_ptr(), i32::from(up), rows) })
    }

    pub fn select(
        &mut self,
        column: i32,
        row: i32,
        start: bool,
        snap: i32,
        rectangular: bool,
        done: bool,
    ) -> Result<(), Error> {
        check(unsafe {
            sys::worminal_engine_select(
                self.handle.as_ptr(),
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

    pub fn selection(&mut self) -> Result<Vec<u8>, Error> {
        let mut size = 0;
        let status = unsafe {
            sys::worminal_engine_selection(self.handle.as_ptr(), std::ptr::null_mut(), 0, &mut size)
        };
        if status != sys::WORMINAL_CAPACITY as i32 {
            check(status)?;
        }
        let mut data = vec![0; size];
        check(unsafe {
            sys::worminal_engine_selection(
                self.handle.as_ptr(),
                data.as_mut_ptr(),
                data.len(),
                &mut size,
            )
        })?;
        Ok(data)
    }

    pub fn clear_selection(&mut self) -> Result<(), Error> {
        check(unsafe { sys::worminal_engine_clear_selection(self.handle.as_ptr()) })
    }

    pub fn selected(&mut self, column: i32, row: i32) -> Result<bool, Error> {
        let mut selected = 0;
        check(unsafe {
            sys::worminal_engine_selected(self.handle.as_ptr(), column, row, &mut selected)
        })?;
        Ok(selected != 0)
    }

    pub fn allow_alternate(&mut self, allowed: bool) -> Result<(), Error> {
        check(unsafe {
            sys::worminal_engine_allow_alternate(self.handle.as_ptr(), i32::from(allowed))
        })
    }

    /// Copies only damaged rows. No C allocation or row pointer escapes this call.
    pub fn changed_rows(&mut self, now: i64) -> Result<(Frame, Vec<(i32, Vec<Cell>)>), Error> {
        self.read_rows(now, false)
    }

    pub fn all_rows(&mut self, now: i64) -> Result<(Frame, Vec<(i32, Vec<Cell>)>), Error> {
        self.read_rows(now, true)
    }

    pub fn frame(&mut self, now: i64) -> Result<Frame, Error> {
        let mut frame = Frame::default();
        check(unsafe { sys::worminal_engine_frame(self.handle.as_ptr(), now, &mut frame) })?;
        Ok(frame)
    }

    pub fn control(&mut self, operation: u32) -> Result<Vec<Event>, Error> {
        check(unsafe { sys::worminal_engine_control(self.handle.as_ptr(), operation) })?;
        self.events()
    }

    fn row_status(&mut self, row: i32) -> Result<(usize, bool), Error> {
        let (mut required, mut dirty) = (0, 0);
        let status = unsafe {
            sys::worminal_engine_row(
                self.handle.as_ptr(),
                row,
                std::ptr::null_mut(),
                0,
                &mut required,
                &mut dirty,
            )
        };
        if status != sys::WORMINAL_CAPACITY as i32 {
            check(status)?;
        }
        Ok((required, dirty != 0))
    }

    pub fn row_dirty(&mut self, row: i32) -> Result<bool, Error> {
        Ok(self.row_status(row)?.1)
    }

    /// Copy into reusable Rust storage without acknowledging damage or borrowing C memory.
    pub fn row(&mut self, row: i32, cells: &mut Vec<Cell>) -> Result<(), Error> {
        let (mut required, _) = self.row_status(row)?;
        cells.resize(required, Cell::default());
        let mut dirty = 0;
        check(unsafe {
            sys::worminal_engine_row(
                self.handle.as_ptr(),
                row,
                cells.as_mut_ptr(),
                cells.len(),
                &mut required,
                &mut dirty,
            )
        })
    }

    /// A replication owner acknowledges damage only after committing its complete queued frame.
    pub fn clean(&mut self) -> Result<(), Error> {
        check(unsafe { sys::worminal_engine_clean(self.handle.as_ptr()) })
    }

    fn read_rows(&mut self, now: i64, full: bool) -> Result<(Frame, Vec<(i32, Vec<Cell>)>), Error> {
        let frame = self.frame(now)?;
        let mut rows = Vec::new();
        if frame.redraw_deadline != 0 {
            return Ok((frame, rows));
        }
        for row in 0..frame.rows {
            if !full && !self.row_dirty(row)? {
                continue;
            }
            let mut cells = Vec::new();
            self.row(row, &mut cells)?;
            rows.push((row, cells));
        }
        self.clean()?;
        Ok((frame, rows))
    }

    pub fn events(&mut self) -> Result<Vec<Event>, Error> {
        let mut result = Vec::new();
        loop {
            let mut event = sys::WorminalEvent::default();
            let status = unsafe {
                sys::worminal_engine_event(
                    self.handle.as_ptr(),
                    &mut event,
                    std::ptr::null_mut(),
                    0,
                )
            };
            if status == sys::WORMINAL_EMPTY as i32 {
                return Ok(result);
            }
            let mut data = Vec::new();
            if status == sys::WORMINAL_CAPACITY as i32 {
                data.resize(event.bytes, 0);
                // An exclusive &mut self prevents consumption or mutation between sizing and copying.
                check(unsafe {
                    sys::worminal_engine_event(
                        self.handle.as_ptr(),
                        &mut event,
                        data.as_mut_ptr(),
                        data.len(),
                    )
                })?;
            } else {
                check(status)?;
            }
            result.push(Event {
                kind: event.type_,
                argument: event.argument,
                flags: event.flags,
                data,
            });
        }
    }

    pub fn export(&mut self, transport: &TransportState) -> Result<Vec<u8>, Error> {
        let mut size = 0;
        let status = unsafe {
            sys::worminal_engine_export(
                self.handle.as_ptr(),
                transport,
                std::ptr::null_mut(),
                0,
                &mut size,
            )
        };
        if status != sys::WORMINAL_CAPACITY as i32 {
            check(status)?;
        }
        let mut data = vec![0; size];
        check(unsafe {
            sys::worminal_engine_export(
                self.handle.as_ptr(),
                transport,
                data.as_mut_ptr(),
                data.len(),
                &mut size,
            )
        })?;
        Ok(data)
    }

    pub fn import(bytes: &[u8]) -> Result<(Self, TransportState), Error> {
        let (engine, transport, _) = Self::import_prefix(bytes)?;
        Ok((engine, transport))
    }

    pub fn import_prefix(bytes: &[u8]) -> Result<(Self, TransportState, usize), Error> {
        let mut handle = std::ptr::null_mut();
        let mut transport = TransportState::default();
        let mut consumed = 0;
        // Failed imports keep ownership in C and publish neither the handle nor partial transport state.
        check(unsafe {
            sys::worminal_engine_import_prefix(
                bytes.as_ptr(),
                bytes.len(),
                &mut handle,
                &mut transport,
                &mut consumed,
            )
        })?;
        let engine = Self {
            handle: NonNull::new(handle).expect("successful import returned null"),
        };
        Ok((engine, transport, consumed))
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // Engine is not Clone; no borrowed C memory or other owner survives destruction.
        unsafe { sys::worminal_engine_destroy(self.handle.as_ptr()) }
    }
}
