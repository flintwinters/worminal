//! Audited SSH spawn boundary. posix_spawnp never runs GUI-process fork hooks.
use std::{
    ffi::{CStr, CString, OsStr},
    io,
    os::{
        fd::{AsFd, AsRawFd, FromRawFd, OwnedFd},
        unix::{ffi::OsStrExt, net::UnixStream},
    },
    ptr, thread,
};
use worminal_protocol::WORKSPACE_DAEMON;

/// Only the local SSH client is signalled. Its identity stays pinned after the waiter reaps its numeric PID.
pub struct Child {
    identity: OwnedFd,
}
impl Child {
    fn signal(&self, signal: libc::c_int) {
        unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.identity.as_raw_fd(),
                signal,
                ptr::null::<libc::siginfo_t>(),
                0,
            );
        }
    }
}
impl Drop for Child {
    fn drop(&mut self) {
        self.signal(libc::SIGTERM);
    }
}
fn reap(pid: libc::pid_t) {
    let _ = crate::spawn::wait(pid);
}

pub fn connect(master: &OsStr) -> io::Result<(UnixStream, Child)> {
    let bytes = master.as_bytes();
    if bytes.is_empty() || bytes[0] == b'-' || bytes.contains(&b'\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid master host",
        ));
    }
    let (parent, remote) = UnixStream::pair()?;
    let daemon = CStr::from_bytes_with_nul(WORKSPACE_DAEMON).expect("generated daemon command");
    let arguments = [
        b"ssh".as_slice(),
        b"-T",
        b"-C",
        b"-oConnectTimeout=10",
        bytes,
        daemon.to_bytes(),
        b"--bridge",
    ]
    .into_iter()
    .map(CString::new)
    .collect::<Result<Vec<_>, _>>()?;
    let pid = crate::spawn::start(
        &arguments,
        &[(remote.as_fd(), 0), (remote.as_fd(), 1)],
        false,
    )?;
    let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if descriptor < 0 {
        let error = io::Error::last_os_error();
        // No waiter exists yet: this unreaped local child still has exclusive numeric-PID ownership.
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
        reap(pid);
        return Err(error);
    }
    let child = Child {
        identity: unsafe { OwnedFd::from_raw_fd(descriptor as i32) },
    };
    if let Err(error) = thread::Builder::new()
        .name("worminal-ssh-reap".into())
        .spawn(move || reap(pid))
    {
        child.signal(libc::SIGKILL);
        reap(pid);
        return Err(error);
    }
    Ok((parent, child))
}
