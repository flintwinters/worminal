//! Audited readiness and signal boundaries; application scheduling remains in the service.
use std::{
    io, mem,
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    ptr,
};

pub(crate) struct Interest {
    pub fd: RawFd,
    pub read: bool,
    pub write: bool,
}
pub(crate) struct Ready {
    pub index: usize,
    pub read: bool,
    pub write: bool,
}

pub(crate) fn wait(interests: &[Interest], timeout_ns: Option<i64>) -> io::Result<Vec<Ready>> {
    let mut descriptors: Vec<_> = interests
        .iter()
        .map(|interest| libc::pollfd {
            fd: interest.fd,
            events: (if interest.read { libc::POLLIN } else { 0 })
                | (if interest.write { libc::POLLOUT } else { 0 }),
            revents: 0,
        })
        .collect();
    let timeout = timeout_ns.map(|ns| libc::timespec {
        tv_sec: ns.max(0) / 1_000_000_000,
        tv_nsec: ns.max(0) % 1_000_000_000,
    });
    let status = unsafe {
        libc::ppoll(
            descriptors.as_mut_ptr(),
            descriptors.len() as libc::nfds_t,
            timeout.as_ref().map_or(ptr::null(), ptr::from_ref),
            ptr::null(),
        )
    };
    if status < 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            return Ok(Vec::new());
        }
        return Err(crate::process::operation("workspace ppoll", error));
    }
    Ok(descriptors
        .iter()
        .enumerate()
        .filter_map(|(index, descriptor)| {
            (descriptor.revents != 0).then_some(Ready {
                index,
                read: descriptor.revents
                    & (libc::POLLIN | libc::POLLHUP | libc::POLLERR | libc::POLLNVAL)
                    != 0,
                write: descriptor.revents & libc::POLLOUT != 0,
            })
        })
        .collect())
}

pub(crate) struct Signals {
    descriptor: OwnedFd,
    original: libc::sigset_t,
}

impl Signals {
    pub(crate) fn new() -> io::Result<Self> {
        unsafe {
            let mut mask: libc::sigset_t = mem::zeroed();
            let mut original: libc::sigset_t = mem::zeroed();
            libc::sigemptyset(&mut mask);
            for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP, libc::SIGCHLD] {
                libc::sigaddset(&mut mask, signal);
            }
            if libc::sigprocmask(libc::SIG_BLOCK, &mask, &mut original) < 0 {
                return Err(io::Error::last_os_error());
            }
            let fd = libc::signalfd(-1, &mask, libc::SFD_CLOEXEC | libc::SFD_NONBLOCK);
            if fd < 0 {
                let error = io::Error::last_os_error();
                libc::sigprocmask(libc::SIG_SETMASK, &original, ptr::null_mut());
                return Err(error);
            }
            Ok(Self {
                descriptor: OwnedFd::from_raw_fd(fd),
                original,
            })
        }
    }

    pub(crate) fn fd(&self) -> RawFd {
        self.descriptor.as_raw_fd()
    }

    pub(crate) fn shutdown_requested(&self) -> io::Result<bool> {
        let mut shutdown = false;
        for _ in 0..64 {
            let mut event = mem::MaybeUninit::<libc::signalfd_siginfo>::uninit();
            let bytes = unsafe {
                libc::read(
                    self.fd(),
                    event.as_mut_ptr().cast(),
                    mem::size_of_val(&event),
                )
            };
            if bytes < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::WouldBlock {
                    break;
                }
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            if bytes as usize != mem::size_of::<libc::signalfd_siginfo>() {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "incomplete service signal record",
                ));
            }
            let event = unsafe { event.assume_init() };
            shutdown |= matches!(
                event.ssi_signo as i32,
                libc::SIGTERM | libc::SIGINT | libc::SIGHUP
            );
        }
        Ok(shutdown)
    }
}

impl Drop for Signals {
    fn drop(&mut self) {
        unsafe {
            libc::sigprocmask(libc::SIG_SETMASK, &self.original, ptr::null_mut());
        }
    }
}
