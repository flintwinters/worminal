//! Shared native syslog boundary; caller modules own event policy and priority.
pub fn log(priority: libc::c_int, message: &str) {
    if let Ok(message) = std::ffi::CString::new(message) {
        unsafe {
            libc::syslog(priority, c"%s".as_ptr().cast(), message.as_ptr());
        }
    }
}
