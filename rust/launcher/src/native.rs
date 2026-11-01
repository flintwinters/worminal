//! Audited GLib and module-loading boundaries. No Rust containers cross the frontend ABI.
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
    include!(concat!(env!("OUT_DIR"), "/frontend.rs"));
}
#[allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]
mod rust_bindings {
    include!(concat!(env!("OUT_DIR"), "/rust_frontend.rs"));
}
use super::Frontend;

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
    frontend: Frontend,
) -> io::Result<()> {
    let executable = std::env::current_exe()?;
    let directory = executable
        .parent()
        .ok_or_else(|| io::Error::other("executable has no directory"))?;
    let (filename, symbol) = match frontend {
        Frontend::Legacy => (
            bindings::WORMINAL_FRONTEND_FILE.as_slice(),
            bindings::WORMINAL_FRONTEND_SYMBOL.as_slice(),
        ),
        Frontend::Rust => (
            rust_bindings::WORMINAL_RUST_FRONTEND_FILE.as_slice(),
            rust_bindings::WORMINAL_RUST_FRONTEND_SYMBOL.as_slice(),
        ),
    };
    let filename = CStr::from_bytes_with_nul(filename).unwrap();
    let filename = std::ffi::OsStr::from_bytes(filename.to_bytes());
    let installed = directory.join("../lib/worminal").join(filename);
    let development = directory.join("build").join(filename);
    let path = if installed.exists() {
        installed
    } else if directory.join(filename).is_file() && !development.is_file() {
        directory.join(filename)
    } else {
        development
    };
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
        // The authoritative C header defines this function pointer. Keep the module resident for
        // asynchronous C callbacks, matching g_module_make_resident in the retained C launcher.
        symbol
    };
    // C may rearrange argv, so provide mutable buffers rather than casting CString::as_ptr().
    let mut storage: Vec<Vec<u8>> = args
        .iter()
        .map(|arg| {
            let mut bytes = arg.as_bytes().to_vec();
            bytes.push(0);
            bytes
        })
        .collect();
    let mut pointers: Vec<*mut libc::c_char> = storage
        .iter_mut()
        .map(|bytes| bytes.as_mut_ptr().cast())
        .collect();
    pointers.push(std::ptr::null_mut());
    let fd = listener.map(IntoRawFd::into_raw_fd).unwrap_or(-1);
    let result = unsafe {
        match frontend {
            Frontend::Legacy => {
                std::mem::transmute::<*mut libc::c_void, bindings::WorminalFrontendEntry>(entry)
                    .unwrap()(args.len() as libc::c_int, pointers.as_mut_ptr(), fd)
            }
            Frontend::Rust => std::mem::transmute::<
                *mut libc::c_void,
                rust_bindings::WorminalRustFrontendEntry,
            >(entry)
            .unwrap()(args.len() as i32, pointers.as_ptr().cast(), fd),
        }
    };
    // Keep argv storage alive through any native exit callbacks. The frontend now owns its listener.
    std::process::exit(result)
}
