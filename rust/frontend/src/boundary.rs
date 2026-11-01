//! Audited module entrypoint. Listener ownership enters before validation; panics never unwind to the launcher.
use std::{
    ffi::{c_char, CStr, OsString},
    io,
    os::{
        fd::FromRawFd,
        unix::{ffi::OsStringExt, net::UnixListener},
    },
    panic::{catch_unwind, AssertUnwindSafe},
};

#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code
)]
mod bindings {
    include!(concat!(env!("OUT_DIR"), "/frontend.rs"));
}

#[no_mangle]
pub unsafe extern "C" fn worminal_rust_frontend_v2(
    argc: i32,
    argv: *const *const c_char,
    listener: i32,
    flags: u32,
) -> i32 {
    let result = catch_unwind(AssertUnwindSafe(|| -> io::Result<()> {
        // The trusted launcher transfers a valid listening descriptor, or -1 for local CLI handling.
        let listener = if listener >= 0 {
            Some(unsafe { UnixListener::from_raw_fd(listener) })
        } else if listener == -1 {
            None
        } else {
            return Err(io::Error::other("invalid listener"));
        };
        if !(1..=65536).contains(&argc) || argv.is_null() {
            return Err(io::Error::other("invalid module arguments"));
        }
        let mut arguments = Vec::with_capacity(argc as usize);
        for index in 0..argc as usize {
            let pointer = unsafe { *argv.add(index) };
            if pointer.is_null() {
                return Err(io::Error::other("null module argument"));
            }
            arguments.push(OsString::from_vec(
                unsafe { CStr::from_ptr(pointer) }.to_bytes().to_vec(),
            ));
        }
        if flags & !bindings::WORMINAL_RUST_FRONTEND_CANDIDATE != 0 {
            return Err(io::Error::other("unsupported frontend flags"));
        }
        crate::app::run_with_listener(arguments, listener, flags != 0)
    }));
    match result {
        Ok(Ok(())) => 0,
        Ok(Err(error)) => {
            eprintln!("worminal: {error}");
            if error.kind() == io::ErrorKind::InvalidInput {
                2
            } else {
                1
            }
        }
        Err(_) => {
            eprintln!("worminal: frontend module operation panicked");
            1
        }
    }
}
// The generated declaration enforces the authoritative header's exact entrypoint signature.
const _: bindings::WorminalRustFrontendEntry = Some(worminal_rust_frontend_v2);
