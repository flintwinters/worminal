//! Audited SSH spawn boundary. posix_spawnp never runs GUI-process fork hooks.
use std::{
    ffi::{CStr, CString, OsStr},
    io,
    mem::MaybeUninit,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{ffi::OsStrExt, net::UnixStream},
    },
    ptr, thread,
};
use worminal_protocol::WORKSPACE_DAEMON;

fn status(value: libc::c_int) -> io::Result<()> {
    if value == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(value))
    }
}
struct Actions(libc::posix_spawn_file_actions_t);
impl Actions {
    fn new() -> io::Result<Self> {
        let mut value = MaybeUninit::uninit();
        status(unsafe { libc::posix_spawn_file_actions_init(value.as_mut_ptr()) })?;
        Ok(Self(unsafe { value.assume_init() }))
    }
}
impl Drop for Actions {
    fn drop(&mut self) {
        unsafe {
            libc::posix_spawn_file_actions_destroy(&mut self.0);
        }
    }
}

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
    loop {
        let result = unsafe { libc::waitpid(pid, ptr::null_mut(), 0) };
        if result >= 0 || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            break;
        }
    }
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
    // A separate high source keeps remapping correct even when stdin or stdout was closed at launch.
    let high = unsafe { libc::fcntl(remote.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
    if high < 0 {
        return Err(io::Error::last_os_error());
    }
    let high = unsafe { OwnedFd::from_raw_fd(high) };
    let mut actions = Actions::new()?;
    for target in [libc::STDIN_FILENO, libc::STDOUT_FILENO] {
        status(unsafe {
            libc::posix_spawn_file_actions_adddup2(&mut actions.0, high.as_raw_fd(), target)
        })?;
    }
    status(unsafe { libc::posix_spawn_file_actions_addclose(&mut actions.0, high.as_raw_fd()) })?;
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
    let mut argv: Vec<_> = arguments
        .iter()
        .map(|value| value.as_ptr().cast_mut())
        .collect();
    argv.push(ptr::null_mut());
    let environment = std::env::vars_os()
        .map(|(key, value)| {
            let mut entry = key.into_encoded_bytes();
            entry.push(b'=');
            entry.extend_from_slice(value.as_bytes());
            CString::new(entry)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut envp: Vec<_> = environment
        .iter()
        .map(|value| value.as_ptr().cast_mut())
        .collect();
    envp.push(ptr::null_mut());
    let mut pid = 0;
    status(unsafe {
        libc::posix_spawnp(
            &mut pid,
            arguments[0].as_ptr(),
            &actions.0,
            ptr::null(),
            argv.as_ptr(),
            envp.as_ptr(),
        )
    })?;
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
