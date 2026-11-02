//! GLib helpers and frontend module loading.
use std::{
    ffi::{CStr, CString, OsString},
    io,
    os::{
        fd::IntoRawFd,
        unix::{ffi::OsStrExt, net::UnixListener},
    },
    path::Path,
};

#[allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]
mod bindings {
    include!(concat!(env!("OUT_DIR"), "/rust_frontend.rs"));
}

unsafe fn glib_bytes(pointer: *mut libc::c_char) -> io::Result<Vec<u8>> {
    if pointer.is_null() {
        return Err(io::Error::other("GLib did not return a value"));
    }
    // Only call with newly allocated GLib strings; copy before releasing their GLib allocation.
    let result = unsafe { CStr::from_ptr(pointer).to_bytes().to_vec() };
    unsafe { glib_sys::g_free(pointer.cast()) };
    Ok(result)
}

pub fn hash(key: &[u8]) -> io::Result<Vec<u8>> {
    unsafe {
        glib_bytes(glib_sys::g_compute_checksum_for_data(
            glib_sys::G_CHECKSUM_SHA256,
            key.as_ptr(),
            key.len(),
        ))
    }
}

pub fn timing(phase: &str) {
    if std::env::var_os("WORMINAL_TRACE_TIMING").is_some() {
        let now = unsafe { glib_sys::g_get_monotonic_time() } * 1000;
        eprintln!("worminal-timing {phase} {now}");
    }
}

pub fn uid() -> u32 {
    unsafe { libc::getuid() }
}

fn loader_error() -> io::Error {
    let message = unsafe { libc::dlerror() };
    let text = if message.is_null() {
        "module symbol unavailable".into()
    } else {
        unsafe { CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned()
    };
    io::Error::other(format!("Cannot load terminal frontend: {text}"))
}

pub fn enter(
    args: Vec<OsString>,
    listener: Option<UnixListener>,
    candidate: bool,
) -> io::Result<()> {
    let executable = std::env::current_exe()?;
    let directory = executable
        .parent()
        .ok_or_else(|| io::Error::other("executable has no directory"))?;
    let filename = bindings::WORMINAL_RUST_FRONTEND_FILE.as_slice();
    let symbol = bindings::WORMINAL_RUST_FRONTEND_SYMBOL.as_slice();
    let filename = CStr::from_bytes_with_nul(filename).unwrap();
    let filename = std::ffi::OsStr::from_bytes(filename.to_bytes());
    let path = directory.join("../lib/worminal").join(filename);
    let path = CString::new(Path::new(&path).as_os_str().as_bytes()).map_err(io::Error::other)?;
    let entry = unsafe {
        let handle = libc::dlopen(path.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL);
        if handle.is_null() {
            return Err(loader_error());
        }
        let symbol = libc::dlsym(handle, symbol.as_ptr().cast());
        if symbol.is_null() {
            let error = loader_error();
            libc::dlclose(handle);
            return Err(error);
        }
        // Use the function type generated from the C header. Keep the library loaded for callbacks
        // until the process exits.
        symbol
    };
    let storage: Vec<CString> = args
        .iter()
        .map(|arg| CString::new(arg.as_bytes()).map_err(io::Error::other))
        .collect::<io::Result<_>>()?;
    let mut pointers: Vec<_> = storage.iter().map(|arg| arg.as_ptr()).collect();
    pointers.push(std::ptr::null());
    let fd = listener.map(IntoRawFd::into_raw_fd).unwrap_or(-1);
    let result = unsafe {
        std::mem::transmute::<*mut libc::c_void, bindings::WorminalRustFrontendEntry>(entry)
            .unwrap()(
            args.len() as i32,
            pointers.as_ptr(),
            fd,
            if candidate {
                bindings::WORMINAL_RUST_FRONTEND_CANDIDATE
            } else {
                0
            },
        )
    };
    // Keep argv storage alive through any native exit callbacks. The frontend now owns its listener.
    std::process::exit(result)
}
