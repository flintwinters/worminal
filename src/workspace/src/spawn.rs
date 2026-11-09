//! Start processes with posix_spawn, without Rust callbacks or GTK fork hooks in the child.
use std::{
    ffi::CString,
    io,
    mem::MaybeUninit,
    os::{
        fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd},
        unix::ffi::OsStrExt,
    },
    ptr,
};

fn status(value: libc::c_int) -> io::Result<()> {
    if value == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(value))
    }
}
struct Actions(libc::posix_spawn_file_actions_t);
impl Drop for Actions {
    fn drop(&mut self) {
        unsafe {
            libc::posix_spawn_file_actions_destroy(&mut self.0);
        }
    }
}
struct Attributes(libc::posix_spawnattr_t);
impl Drop for Attributes {
    fn drop(&mut self) {
        unsafe {
            libc::posix_spawnattr_destroy(&mut self.0);
        }
    }
}

/// The caller exclusively owns this unreaped local child and must arrange its exact-PID wait.
pub fn start(
    arguments: &[CString],
    stdio: &[(BorrowedFd<'_>, i32)],
    independent_session: bool,
) -> io::Result<libc::pid_t> {
    if arguments.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "missing program",
        ));
    }
    let mut actions = MaybeUninit::uninit();
    status(unsafe { libc::posix_spawn_file_actions_init(actions.as_mut_ptr()) })?;
    let mut actions = Actions(unsafe { actions.assume_init() });
    let mut attributes = MaybeUninit::uninit();
    status(unsafe { libc::posix_spawnattr_init(attributes.as_mut_ptr()) })?;
    let mut attributes = Attributes(unsafe { attributes.assume_init() });
    if independent_session {
        status(unsafe {
            libc::posix_spawnattr_setflags(&mut attributes.0, libc::POSIX_SPAWN_SETSID)
        })?;
    }
    // Duplicate all sources above stdio before remapping any destination, including closed-stdio launches.
    let sources: Vec<_> = stdio
        .iter()
        .map(|(source, destination)| {
            if !(0..=2).contains(destination) {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid stdio"));
            }
            let high = unsafe { libc::fcntl(source.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
            if high < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok((unsafe { OwnedFd::from_raw_fd(high) }, *destination))
        })
        .collect::<io::Result<_>>()?;
    for (source, destination) in &sources {
        status(unsafe {
            libc::posix_spawn_file_actions_adddup2(&mut actions.0, source.as_raw_fd(), *destination)
        })?;
    }
    for (source, _) in &sources {
        status(unsafe {
            libc::posix_spawn_file_actions_addclose(&mut actions.0, source.as_raw_fd())
        })?;
    }
    let environment: Vec<_> = std::env::vars_os()
        .map(|(key, value)| {
            let mut entry = key.into_encoded_bytes();
            entry.push(b'=');
            entry.extend_from_slice(value.as_bytes());
            CString::new(entry)
        })
        .collect::<Result<_, _>>()?;
    let pointers = |values: &[CString]| {
        let mut result: Vec<_> = values
            .iter()
            .map(|value| value.as_ptr().cast_mut())
            .collect();
        result.push(ptr::null_mut());
        result
    };
    let argv = pointers(arguments);
    let envp = pointers(&environment);
    let mut pid = 0;
    status(unsafe {
        libc::posix_spawnp(
            &mut pid,
            arguments[0].as_ptr(),
            &actions.0,
            &attributes.0,
            argv.as_ptr(),
            envp.as_ptr(),
        )
    })?;
    Ok(pid)
}
pub fn wait(pid: libc::pid_t) -> io::Result<i32> {
    if pid <= 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "wait requires an owned child PID",
        ));
    }
    loop {
        let mut status = 0;
        let result = unsafe { libc::waitpid(pid, &mut status, 0) };
        if result == pid {
            return Ok(status);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

/// A local child whose identity survives waiter reaping. Drop signals only this exact process.
pub struct Child {
    pid: libc::pid_t,
    identity: OwnedFd,
    exit: std::sync::mpsc::Receiver<io::Result<i32>>,
}
impl Child {
    pub fn pid(&self) -> libc::pid_t {
        self.pid
    }
    pub fn signal(&self, signal: libc::c_int) {
        unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.identity.as_raw_fd(),
                signal,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            );
        }
    }
    pub fn exited(&self) -> bool {
        let mut poll = libc::pollfd {
            fd: self.identity.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        !matches!(
            self.exit.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ) || unsafe { libc::poll(&mut poll, 1, 0) } != 0
    }
}
impl Drop for Child {
    fn drop(&mut self) {
        self.signal(libc::SIGTERM);
    }
}

pub fn own(pid: libc::pid_t, name: &str) -> io::Result<Child> {
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd < 0 {
        let error = io::Error::last_os_error();
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
        let _ = wait(pid);
        return Err(error);
    }
    let (send, exit) = std::sync::mpsc::sync_channel(1);
    let child = Child {
        pid,
        identity: unsafe { OwnedFd::from_raw_fd(fd as i32) },
        exit,
    };
    if let Err(error) = std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let _ = send.send(wait(pid));
        })
    {
        child.signal(libc::SIGKILL);
        let _ = wait(pid);
        return Err(error);
    }
    Ok(child)
}
