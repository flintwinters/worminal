//! Audited Linux ownership-transfer boundary. A sealed read-only mapping owns its backing descriptor.
use crate::process;
use std::{
    fs::File,
    io::{self, Seek, SeekFrom},
    mem,
    os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd},
    ptr::NonNull,
};
use worminal_workspace::handoff::{HANDOFF_SNAPSHOT_LIMIT, HANDOFF_TIMEOUT_SECONDS};

const SEALS: libc::c_int =
    libc::F_SEAL_WRITE | libc::F_SEAL_GROW | libc::F_SEAL_SHRINK | libc::F_SEAL_SEAL;

fn checked(status: libc::c_int) -> io::Result<libc::c_int> {
    if status < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(status)
    }
}

fn address(name: &[u8]) -> io::Result<(libc::sockaddr_un, libc::socklen_t)> {
    if name.is_empty() || name.len() > 60 || name.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid handoff name",
        ));
    }
    let mut address: libc::sockaddr_un = unsafe { mem::zeroed() };
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    let mut endpoint = format!("worminal-handoff-{}-", process::uid()).into_bytes();
    endpoint.extend_from_slice(name);
    if endpoint.len() >= address.sun_path.len() - 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "handoff address is too long",
        ));
    }
    for (target, byte) in address.sun_path[1..].iter_mut().zip(&endpoint) {
        *target = *byte as libc::c_char;
    }
    Ok((
        address,
        (mem::offset_of!(libc::sockaddr_un, sun_path) + 1 + endpoint.len()) as libc::socklen_t,
    ))
}

fn socket() -> io::Result<OwnedFd> {
    let fd = checked(unsafe {
        libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0)
    })?;
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn configure(fd: RawFd) -> io::Result<()> {
    process::peer_pid(fd)?;
    let timeout = libc::timeval {
        tv_sec: HANDOFF_TIMEOUT_SECONDS as libc::time_t,
        tv_usec: 0,
    };
    for option in [libc::SO_RCVTIMEO, libc::SO_SNDTIMEO] {
        checked(unsafe {
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                option,
                (&timeout as *const libc::timeval).cast(),
                mem::size_of_val(&timeout) as libc::socklen_t,
            )
        })?;
    }
    Ok(())
}

pub(crate) fn inherited_descriptor(fd: RawFd) -> io::Result<OwnedFd> {
    let flags = checked(unsafe { libc::fcntl(fd, libc::F_GETFD) })?;
    // Reserved descriptors belong to the fresh receiver executable, with no other Rust owner.
    let descriptor = unsafe { OwnedFd::from_raw_fd(fd) };
    checked(unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) })?;
    Ok(descriptor)
}

pub(crate) fn inherited_listener(fd: RawFd, kind: libc::c_int) -> io::Result<OwnedFd> {
    let descriptor = inherited_descriptor(fd)?;
    for (option, expected) in [(libc::SO_ACCEPTCONN, 1), (libc::SO_TYPE, kind)] {
        let mut actual = 0i32;
        let mut length = mem::size_of_val(&actual) as libc::socklen_t;
        checked(unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                option,
                (&mut actual as *mut i32).cast(),
                &mut length,
            )
        })?;
        if actual != expected || length as usize != mem::size_of_val(&actual) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected inherited listener",
            ));
        }
    }
    let mut address: libc::sockaddr_storage = unsafe { mem::zeroed() };
    let mut length = mem::size_of_val(&address) as libc::socklen_t;
    checked(unsafe {
        libc::getsockname(
            fd,
            (&mut address as *mut libc::sockaddr_storage).cast(),
            &mut length,
        )
    })?;
    if address.ss_family != libc::AF_UNIX as libc::sa_family_t {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "inherited listener is not local",
        ));
    }
    Ok(descriptor)
}

pub(crate) struct Listener(OwnedFd);
impl Listener {
    pub fn inherited(fd: RawFd) -> io::Result<Self> {
        let descriptor = inherited_listener(fd, libc::SOCK_SEQPACKET)?;
        process::nonblocking(descriptor.as_raw_fd())?;
        Ok(Self(descriptor))
    }
    pub fn bind(name: &[u8]) -> io::Result<Self> {
        let (address, length) = address(name)?;
        let fd = socket()?;
        checked(unsafe {
            libc::bind(
                fd.as_raw_fd(),
                (&address as *const libc::sockaddr_un).cast(),
                length,
            )
        })?;
        checked(unsafe { libc::listen(fd.as_raw_fd(), 1) })?;
        process::nonblocking(fd.as_raw_fd())?;
        Ok(Self(fd))
    }
    pub fn accept(&self) -> io::Result<Channel> {
        let fd = checked(unsafe {
            libc::accept4(
                self.as_raw_fd(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                libc::SOCK_CLOEXEC,
            )
        })?;
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        configure(fd.as_raw_fd())?;
        Ok(Channel(fd))
    }
}
impl AsRawFd for Listener {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}
impl AsFd for Listener {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

pub(crate) struct CommitSent {
    _private: (),
}
pub(crate) struct CommitReceived {
    _private: (),
}
pub(crate) struct Channel(OwnedFd);
impl Channel {
    pub fn connect(name: &[u8]) -> io::Result<Self> {
        let (address, length) = address(name)?;
        let fd = socket()?;
        // Bound even a connect to a congested same-user listener; parsing remains frozen only briefly.
        process::nonblocking(fd.as_raw_fd())?;
        let status = unsafe {
            libc::connect(
                fd.as_raw_fd(),
                (&address as *const libc::sockaddr_un).cast(),
                length,
            )
        };
        if status < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EINPROGRESS) {
                return Err(error);
            }
            let mut ready = libc::pollfd {
                fd: fd.as_raw_fd(),
                events: libc::POLLOUT,
                revents: 0,
            };
            checked(unsafe { libc::poll(&mut ready, 1, (HANDOFF_TIMEOUT_SECONDS * 1000) as i32) })?;
            if ready.revents == 0 {
                return Err(io::ErrorKind::TimedOut.into());
            }
            let mut error = 0i32;
            let mut size = mem::size_of_val(&error) as libc::socklen_t;
            checked(unsafe {
                libc::getsockopt(
                    fd.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_ERROR,
                    (&mut error as *mut i32).cast(),
                    &mut size,
                )
            })?;
            if error != 0 {
                return Err(io::Error::from_raw_os_error(error));
            }
        }
        let flags = checked(unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) })?;
        checked(unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags & !libc::O_NONBLOCK) })?;
        configure(fd.as_raw_fd())?;
        Ok(Self(fd))
    }

    fn send(&self, marker: u8) -> io::Result<()> {
        let status = unsafe {
            libc::send(
                self.as_raw_fd(),
                (&marker as *const u8).cast(),
                1,
                libc::MSG_NOSIGNAL,
            )
        };
        if status == 1 {
            Ok(())
        } else if status < 0 {
            Err(io::Error::last_os_error())
        } else {
            Err(io::ErrorKind::WriteZero.into())
        }
    }
    fn expect(&self, expected: u8) -> io::Result<()> {
        let mut marker = 0u8;
        let status = unsafe { libc::recv(self.as_raw_fd(), (&mut marker as *mut u8).cast(), 1, 0) };
        if status < 0 {
            return Err(io::Error::last_os_error());
        }
        if status == 1 && marker == expected {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "handoff marker was not received",
            ))
        }
    }
    pub fn ready(&self) -> io::Result<()> {
        self.send(b'R')
    }
    pub fn commit(&self) -> io::Result<CommitSent> {
        self.expect(b'R')?;
        self.send(b'C')?;
        Ok(CommitSent { _private: () })
    }
    pub fn accept_commit(&self) -> io::Result<CommitReceived> {
        self.expect(b'C')?;
        Ok(CommitReceived { _private: () })
    }
    pub fn acknowledge(&self) -> io::Result<()> {
        self.send(b'A')
    }
    pub fn acknowledgement(&self) -> io::Result<()> {
        self.expect(b'A')
    }

    #[cfg(test)]
    pub(crate) fn pair() -> io::Result<(Self, Self)> {
        let mut fds = [0; 2];
        checked(unsafe {
            libc::socketpair(
                libc::AF_UNIX,
                libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
                0,
                fds.as_mut_ptr(),
            )
        })?;
        let (a, b) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        configure(a.as_raw_fd())?;
        configure(b.as_raw_fd())?;
        Ok((Self(a), Self(b)))
    }
}
impl AsRawFd for Channel {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}
impl AsFd for Channel {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

pub(crate) fn snapshot() -> io::Result<File> {
    let fd = checked(unsafe {
        libc::memfd_create(
            b"worminal-handoff\0".as_ptr().cast(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        )
    })?;
    Ok(File::from(unsafe { OwnedFd::from_raw_fd(fd) }))
}
pub(crate) fn seal(file: &mut File) -> io::Result<()> {
    checked(unsafe { libc::fcntl(file.as_raw_fd(), libc::F_ADD_SEALS, SEALS) })?;
    file.seek(SeekFrom::Start(0))?;
    Ok(())
}

pub(crate) struct Mapping {
    address: NonNull<u8>,
    length: usize,
    _file: File,
}
impl Mapping {
    pub fn new(descriptor: OwnedFd) -> io::Result<Self> {
        let file = File::from(descriptor);
        let info = file.metadata()?;
        let seals = checked(unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GET_SEALS) })?;
        if !info.is_file()
            || info.len() < 12
            || info.len() > u64::from(HANDOFF_SNAPSHOT_LIMIT)
            || seals & SEALS != SEALS
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "handoff snapshot is not bounded and sealed",
            ));
        }
        let length = info.len() as usize;
        let address = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                length,
                libc::PROT_READ,
                libc::MAP_PRIVATE,
                file.as_raw_fd(),
                0,
            )
        };
        if address == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let Some(address) = NonNull::new(address.cast::<u8>()) else {
            unsafe {
                libc::munmap(address, length);
            }
            return Err(io::Error::other(
                "snapshot mapped at an unsupported null address",
            ));
        };
        Ok(Self {
            address,
            length,
            _file: file,
        })
    }
    pub fn bytes(&self) -> &[u8] {
        // Seals prevent mutation/truncation; this immutable slice cannot outlive its mapping.
        unsafe { std::slice::from_raw_parts(self.address.as_ptr(), self.length) }
    }
}
impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.address.as_ptr().cast(), self.length);
        }
    }
}
