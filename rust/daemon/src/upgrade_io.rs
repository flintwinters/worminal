//! Watch executable replacements, receive file descriptors and start replacement daemons.
use crate::handoff_io::{self, CommitSent, Listener};
use std::os::unix::fs::FileExt;
use std::{
    ffi::{CString, OsString},
    fs::{self, File},
    io::{self, Read},
    mem,
    os::{
        fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd},
        unix::{
            ffi::{OsStrExt, OsStringExt},
            fs::MetadataExt,
            net::UnixListener,
        },
    },
    path::PathBuf,
};
use worminal_workspace::handoff::{HANDOFF_MAX_TABS, HANDOFF_TIMEOUT_SECONDS};

fn checked(result: libc::c_int) -> io::Result<libc::c_int> {
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result)
    }
}
fn spawn_status(result: libc::c_int) -> io::Result<()> {
    if result != 0 {
        Err(io::Error::from_raw_os_error(result))
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Identity {
    device: u64,
    inode: u64,
}
impl Identity {
    fn of(info: &fs::Metadata) -> Self {
        Self {
            device: info.dev(),
            inode: info.ino(),
        }
    }
}
pub(crate) struct Replacement {
    pub path: PathBuf,
    pub inode: u64,
}
pub(crate) struct Watch {
    file: File,
    path: PathBuf,
    loaded: Identity,
    attempted: Option<Identity>,
    refresh_needed: bool,
}
impl Watch {
    pub fn new() -> io::Result<Self> {
        let loaded = Identity::of(&fs::metadata("/proc/self/exe")?);
        let mut bytes = fs::read_link("/proc/self/exe")?.into_os_string().into_vec();
        if bytes.ends_with(b" (deleted)") {
            bytes.truncate(bytes.len() - b" (deleted)".len());
        }
        let path = PathBuf::from(OsString::from_vec(bytes));
        let directory = path
            .parent()
            .ok_or_else(|| io::Error::other("executable has no directory"))?;
        let name = CString::new(directory.as_os_str().as_bytes())?;
        let fd = checked(unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) })?;
        let file = File::from(unsafe { OwnedFd::from_raw_fd(fd) });
        checked(unsafe {
            libc::inotify_add_watch(
                fd,
                name.as_ptr(),
                libc::IN_MOVED_TO | libc::IN_CLOSE_WRITE | libc::IN_ATTRIB,
            )
        })?;
        Ok(Self {
            file,
            path,
            loaded,
            attempted: None,
            refresh_needed: true,
        })
    }

    pub fn replacement(&mut self, allowed: bool) -> io::Result<Option<Replacement>> {
        let mut changed = mem::take(&mut self.refresh_needed);
        let mut bytes = [0u8; 8192];
        // Directory activity cannot starve existing clients. Unread events retain readiness for the next turn.
        for _ in 0..64 {
            let count = match self.file.read(&mut bytes) {
                Ok(0) => break,
                Ok(count) => count,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            let mut offset = 0;
            let header = mem::size_of::<libc::inotify_event>();
            while count - offset >= header {
                // Kernel events need not begin at a Rust byte array's natural alignment.
                let event = unsafe {
                    bytes
                        .as_ptr()
                        .add(offset)
                        .cast::<libc::inotify_event>()
                        .read_unaligned()
                };
                let end = offset
                    .checked_add(header)
                    .and_then(|start| start.checked_add(event.len as usize))
                    .filter(|end| *end <= count)
                    .ok_or_else(|| io::Error::other("invalid executable-watch event"))?;
                let name = bytes[offset + header..end]
                    .split(|byte| *byte == 0)
                    .next()
                    .unwrap();
                changed |= event.mask & libc::IN_Q_OVERFLOW != 0
                    || self
                        .path
                        .file_name()
                        .is_some_and(|filename| filename.as_bytes() == name);
                offset = end;
            }
        }
        if !allowed {
            self.refresh_needed = changed;
            return Ok(None);
        }
        if !changed {
            return Ok(None);
        }
        let info = match fs::metadata(&self.path) {
            Ok(info) => info,
            Err(_) => return Ok(None),
        };
        let identity = Identity::of(&info);
        if identity == self.loaded || self.attempted == Some(identity) {
            return Ok(None);
        }
        self.attempted = Some(identity);
        if !info.is_file() {
            return Ok(None);
        }
        Ok(Some(Replacement {
            path: self.path.clone(),
            inode: identity.inode,
        }))
    }
}
impl AsRawFd for Watch {
    fn as_raw_fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }
}

struct Actions(libc::posix_spawn_file_actions_t);
impl Actions {
    fn new() -> io::Result<Self> {
        let mut actions = mem::MaybeUninit::uninit();
        spawn_status(unsafe { libc::posix_spawn_file_actions_init(actions.as_mut_ptr()) })?;
        Ok(Self(unsafe { actions.assume_init() }))
    }
}
impl Drop for Actions {
    fn drop(&mut self) {
        unsafe {
            libc::posix_spawn_file_actions_destroy(&mut self.0);
        }
    }
}

pub(crate) struct ReceiverChild {
    pub pid: libc::pid_t,
    reaped: bool,
}
impl ReceiverChild {
    pub fn spawn(
        path: &std::path::Path,
        listener: BorrowedFd<'_>,
        receiving: BorrowedFd<'_>,
    ) -> io::Result<Self> {
        let mut actions = Actions::new()?;
        let mut copies = Vec::new();
        for (index, source) in [listener, receiving].iter().enumerate() {
            // High copies keep dup2(3,4) from overwriting either source, including a low-numbered listener.
            let copy =
                checked(unsafe { libc::fcntl(source.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 64) })?;
            copies.push(unsafe { OwnedFd::from_raw_fd(copy) });
            spawn_status(unsafe {
                libc::posix_spawn_file_actions_adddup2(&mut actions.0, copy, 3 + index as i32)
            })?;
        }
        let path = CString::new(path.as_os_str().as_bytes())?;
        let argument = CString::new("--upgrade-receiver").unwrap();
        let arguments = [
            path.as_ptr().cast_mut(),
            argument.as_ptr().cast_mut(),
            std::ptr::null_mut(),
        ];
        let environment: Vec<_> = std::env::vars_os()
            .map(|(key, value)| {
                let mut entry = key.into_vec();
                entry.push(b'=');
                entry.extend_from_slice(value.as_bytes());
                CString::new(entry)
            })
            .collect::<Result<_, _>>()?;
        let mut entries: Vec<_> = environment
            .iter()
            .map(|entry| entry.as_ptr().cast_mut())
            .collect();
        entries.push(std::ptr::null_mut());
        let mut pid = 0;
        // No Rust closure or GUI fork hook runs in the child. libc performs the explicit descriptor actions.
        spawn_status(unsafe {
            libc::posix_spawn(
                &mut pid,
                path.as_ptr(),
                &actions.0,
                std::ptr::null(),
                arguments.as_ptr(),
                entries.as_ptr(),
            )
        })?;
        Ok(Self { pid, reaped: false })
    }

    fn poll(&mut self) -> io::Result<bool> {
        if self.reaped {
            return Ok(true);
        }
        let status = loop {
            let status = unsafe { libc::waitpid(self.pid, std::ptr::null_mut(), libc::WNOHANG) };
            if status >= 0 || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                break status;
            }
        };
        if status == self.pid {
            self.reaped = true;
            return Ok(true);
        }
        if status == 0 {
            return Ok(false);
        }
        // A wait error loses numeric-PID authority, just as for ordinary local shell children.
        self.reaped = true;
        Err(io::Error::last_os_error())
    }
    pub fn reject(mut self) -> io::Result<()> {
        if self.poll()? {
            return Ok(());
        }
        // Only this object waits for this unreaped local child; no global waitpid(-1) may steal its identity.
        checked(unsafe { libc::kill(self.pid, libc::SIGKILL) })?;
        loop {
            let status = unsafe { libc::waitpid(self.pid, std::ptr::null_mut(), 0) };
            if status == self.pid {
                self.reaped = true;
                return Ok(());
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
    pub fn committed(self, _commit: &CommitSent) {}
}
impl Drop for ReceiverChild {
    fn drop(&mut self) {
        // Never signal on drop: C may already have crossed the ownership boundary before the caller regains control.
        let _ = self.poll();
    }
}

pub(crate) struct Manifest {
    pub ids: Vec<u32>,
    pub next_id: u32,
    pub retiring_pid: i32,
}
impl Manifest {
    pub fn inherited() -> io::Result<Self> {
        let file = File::from(handoff_io::inherited_descriptor(5)?);
        let info = file.metadata()?;
        let maximum = u64::from(HANDOFF_MAX_TABS + 3) * 4;
        if !info.is_file() || info.len() < 12 || info.len() > maximum || info.len() % 4 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid legacy identity manifest",
            ));
        }
        let mut bytes = vec![0; info.len() as usize];
        file.read_exact_at(&mut bytes, 0)?;
        let fields: Vec<_> = bytes
            .chunks_exact(4)
            .map(|bytes| u32::from_ne_bytes(bytes.try_into().unwrap()))
            .collect();
        if fields[0] > HANDOFF_MAX_TABS
            || fields[0] as usize + 3 != fields.len()
            || fields[2] > i32::MAX as u32
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid legacy identity count",
            ));
        }
        Ok(Self {
            ids: fields[3..].to_vec(),
            next_id: fields[1],
            retiring_pid: fields[2] as i32,
        })
    }
}

pub(crate) fn inherited_listeners() -> io::Result<(UnixListener, Listener)> {
    let listener = UnixListener::from(handoff_io::inherited_listener(3, libc::SOCK_STREAM)?);
    listener.set_nonblocking(true)?;
    let receiving = Listener::inherited(4)?;
    Ok((listener, receiving))
}

pub(crate) fn wait_receiver(listener: &Listener) -> io::Result<()> {
    let interests = [crate::reactor::Interest {
        fd: listener.as_raw_fd(),
        read: true,
        write: false,
    }];
    let deadline = std::time::Instant::now()
        + std::time::Duration::from_secs(u64::from(HANDOFF_TIMEOUT_SECONDS));
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        if !crate::reactor::wait(&interests, Some(left.as_nanos() as i64))?.is_empty() {
            return Ok(());
        }
    }
}
