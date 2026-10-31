//! Audited PTY, child identity, and signal boundary. No GTK or terminal-parser state lives here.
use std::{
    cell::RefCell,
    collections::BTreeMap,
    ffi::{CStr, OsString},
    fs::{File, OpenOptions},
    io, mem,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::{
            ffi::{OsStrExt, OsStringExt},
            process::CommandExt,
        },
    },
    path::PathBuf,
    process::{Child, Command, Stdio},
    ptr,
    rc::Rc,
};
use worminal_workspace::{launch::Launch, PROCESS_SHELL, PROCESS_STTY, PROCESS_TERM_NAME};

fn checked(result: libc::c_int) -> io::Result<libc::c_int> {
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result)
    }
}

fn log(priority: libc::c_int, message: &str) {
    if let Ok(message) = std::ffi::CString::new(message) {
        unsafe {
            libc::syslog(priority, b"%s\0".as_ptr().cast(), message.as_ptr());
        }
    }
}

pub fn diagnostic(message: &str) {
    log(libc::LOG_NOTICE, message);
}
pub fn error(message: &str) {
    log(libc::LOG_ERR, message);
}

pub(crate) fn operation(name: &str, error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!(
            "{name} failed: {error} (errno {})",
            error.raw_os_error().unwrap_or(0)
        ),
    )
}

/// Call once in the headless executable, before any parser, thread, or signal registration exists.
pub fn prepare_service() -> io::Result<()> {
    unsafe {
        libc::openlog(
            b"worminald\0".as_ptr().cast(),
            libc::LOG_PID,
            libc::LOG_USER,
        );
        if libc::setlocale(libc::LC_CTYPE, b"\0".as_ptr().cast()).is_null() {
            return Err(io::Error::other(
                "cannot initialize terminal character locale",
            ));
        }
        if libc::getsid(0) != libc::getpid() {
            checked(libc::setsid())?;
        }
    }
    Ok(())
}

pub(crate) fn ignore_hangup() {
    unsafe {
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
    }
}

pub(crate) fn detached_command(executable: &std::path::Path) -> Command {
    let mut command = Command::new(executable);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Only used by the standalone, headless bridge, never by GTK or a module callback.
    unsafe {
        command.pre_exec(|| {
            checked(libc::setsid())?;
            Ok(())
        });
    }
    command
}

/// The standalone bridge owns these process-global descriptors until it exits.
pub(crate) struct StdioDescriptor(RawFd);
impl StdioDescriptor {
    pub(crate) fn new(fd: RawFd) -> io::Result<Self> {
        if fd != 0 && fd != 1 {
            return Err(io::Error::other("invalid bridge stdio descriptor"));
        }
        nonblocking(fd)?;
        Ok(Self(fd))
    }
}
impl io::Read for StdioDescriptor {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let count = unsafe { libc::read(self.0, bytes.as_mut_ptr().cast(), bytes.len()) };
        if count < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(count as usize)
        }
    }
}
impl io::Write for StdioDescriptor {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let count = unsafe { libc::write(self.0, bytes.as_ptr().cast(), bytes.len()) };
        if count < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(count as usize)
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn os_constant(bytes: &[u8]) -> OsString {
    OsString::from_vec(
        CStr::from_bytes_with_nul(bytes)
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
}

pub fn uid() -> u32 {
    unsafe { libc::getuid() }
}

pub fn nonblocking(fd: RawFd) -> io::Result<()> {
    unsafe {
        let flags = checked(libc::fcntl(fd, libc::F_GETFL))?;
        checked(libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK))?;
        checked(libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC))?;
    }
    Ok(())
}

fn open_pty(columns: u16, rows: u16) -> io::Result<(File, File)> {
    unsafe {
        // CLOEXEC must be atomic with creation: another thread may spawn while this PTY is prepared.
        let descriptor = checked(libc::posix_openpt(
            libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC | libc::O_NONBLOCK,
        ))
        .map_err(|error| operation("posix_openpt", error))?;
        let master = File::from_raw_fd(descriptor);
        checked(libc::grantpt(master.as_raw_fd()))?;
        checked(libc::unlockpt(master.as_raw_fd()))?;
        let mut name = vec![0u8; 128];
        loop {
            let result = libc::ptsname_r(master.as_raw_fd(), name.as_mut_ptr().cast(), name.len());
            if result == libc::ERANGE && name.len() < 4096 {
                name.resize(name.len() * 2, 0);
                continue;
            }
            if result != 0 {
                return Err(io::Error::from_raw_os_error(result));
            }
            break;
        }
        let descriptor = checked(libc::open(
            name.as_ptr().cast(),
            libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
        ))?;
        let slave = File::from_raw_fd(descriptor);
        let size = libc::winsize {
            ws_row: rows,
            ws_col: columns,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        checked(libc::ioctl(slave.as_raw_fd(), libc::TIOCSWINSZ, &size))?;
        Ok((master, slave))
    }
}

pub fn peer_pid(fd: RawFd) -> io::Result<libc::pid_t> {
    unsafe {
        let mut credentials: libc::ucred = mem::zeroed();
        let mut size = mem::size_of_val(&credentials) as libc::socklen_t;
        checked(libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            ptr::from_mut(&mut credentials).cast(),
            &mut size,
        ))?;
        if size as usize != mem::size_of_val(&credentials) || credentials.uid != uid() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "workspace peer is not the same user",
            ));
        }
        Ok(credentials.pid)
    }
}

pub fn session_of(pid: libc::pid_t) -> io::Result<libc::pid_t> {
    checked(unsafe { libc::getsid(pid) })
}

pub(crate) fn restore_directory(directory: &[u8]) -> Vec<u8> {
    let usable = |directory: &[u8]| {
        let Ok(path) = std::ffi::CString::new(directory) else {
            return false;
        };
        std::fs::metadata(std::ffi::OsStr::from_bytes(directory)).is_ok_and(|info| info.is_dir())
            && unsafe { libc::access(path.as_ptr(), libc::X_OK) } == 0
    };
    if usable(directory) {
        return directory.to_vec();
    }
    let home = std::env::var_os("HOME").unwrap_or_default().into_vec();
    if usable(&home) {
        home
    } else {
        b"/".to_vec()
    }
}

struct User {
    name: OsString,
    home: OsString,
    shell: OsString,
}

fn user() -> io::Result<User> {
    let mut buffer = vec![0u8; 16384];
    loop {
        unsafe {
            let mut entry: libc::passwd = mem::zeroed();
            let mut result = ptr::null_mut();
            let status = libc::getpwuid_r(
                uid(),
                &mut entry,
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            );
            if status == libc::ERANGE && buffer.len() < 1024 * 1024 {
                buffer.resize(buffer.len() * 2, 0);
                continue;
            }
            if status != 0 {
                return Err(io::Error::from_raw_os_error(status));
            }
            if result.is_null() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "current user has no passwd entry",
                ));
            }
            // Copy every passwd string before its caller-owned backing buffer is released or reused.
            let copy = |value: *const libc::c_char| -> io::Result<OsString> {
                let offset = (value as usize)
                    .checked_sub(buffer.as_ptr() as usize)
                    .filter(|offset| *offset < buffer.len())
                    .ok_or_else(|| {
                        io::Error::other("passwd field is outside its backing buffer")
                    })?;
                let text = CStr::from_bytes_until_nul(&buffer[offset..])
                    .map_err(|_| io::Error::other("passwd field is not terminated"))?;
                Ok(OsString::from_vec(text.to_bytes().to_vec()))
            };
            return Ok(User {
                name: copy(entry.pw_name)?,
                home: copy(entry.pw_dir)?,
                shell: copy(entry.pw_shell)?,
            });
        }
    }
}

pub struct PinnedProcess {
    pid: libc::pid_t,
    handle: Option<OwnedFd>,
}

impl PinnedProcess {
    /// A transferred process is not a child. Never signal its numeric PID after identity reuse.
    pub fn prepare(pid: libc::pid_t) -> io::Result<Self> {
        if pid <= 0 {
            return Ok(Self {
                pid: 0,
                handle: None,
            });
        }
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as libc::c_int;
        if fd < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                return Ok(Self {
                    pid: 0,
                    handle: None,
                });
            }
            return Err(error);
        }
        // pidfd_open returned a new owned descriptor. It is closed exactly once by OwnedFd.
        Ok(Self {
            pid,
            handle: Some(unsafe { OwnedFd::from_raw_fd(fd) }),
        })
    }

    pub fn live_pid(&self) -> libc::pid_t {
        let Some(handle) = &self.handle else {
            return 0;
        };
        let mut ready = libc::pollfd {
            fd: handle.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        if unsafe { libc::poll(&mut ready, 1, 0) } > 0 {
            0
        } else {
            self.pid
        }
    }

    pub fn signal(&self, signal: libc::c_int) -> io::Result<()> {
        let Some(handle) = &self.handle else {
            return Ok(());
        };
        let result = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                handle.as_raw_fd(),
                signal,
                ptr::null::<libc::siginfo_t>(),
                0,
            )
        } as libc::c_int;
        if result < 0 && io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
            checked(result)?;
        }
        Ok(())
    }
}

enum Identity {
    Local(Child),
    Transferred(PinnedProcess),
    None,
}

#[derive(Clone, Default)]
pub struct ChildReaper(Rc<RefCell<Vec<Child>>>);

impl ChildReaper {
    pub fn reap(&self) -> usize {
        self.0
            .borrow_mut()
            .retain_mut(|child| matches!(child.try_wait(), Ok(None)));
        self.0.borrow().len()
    }

    fn retain(&self, child: Child) {
        self.0.borrow_mut().push(child);
        self.reap();
    }
}

pub struct Pty {
    master: Option<File>,
    identity: Identity,
    reaper: ChildReaper,
}

impl Pty {
    pub fn spawn(launch: &Launch, restored: bool, reaper: ChildReaper) -> io::Result<Self> {
        let directory = if launch.directory.is_empty() {
            std::env::var_os("HOME").unwrap_or_else(|| "/".into())
        } else {
            OsString::from_vec(launch.directory.clone())
        };
        if !launch.line.is_empty() {
            let line = PathBuf::from(&directory).join(OsString::from_vec(launch.line.clone()));
            let master = OpenOptions::new().read(true).write(true).open(line)?;
            let mut configuration = os_constant(PROCESS_STTY).into_vec();
            for arg in &launch.arguments {
                configuration.push(b' ');
                configuration.extend_from_slice(arg.as_bytes());
            }
            let mut command = Command::new(os_constant(PROCESS_SHELL));
            command
                .arg("-c")
                .arg(OsString::from_vec(configuration))
                .current_dir(&directory)
                .stdin(Stdio::from(master.try_clone()?));
            if let Some(environment) = &launch.environment {
                command.env_clear().envs(environment.iter().cloned());
            }
            command.status()?;
            nonblocking(master.as_raw_fd())?;
            return Ok(Self {
                master: Some(master),
                identity: Identity::None,
                reaper,
            });
        }
        let account = user()?;
        let mut environment: BTreeMap<OsString, OsString> = if restored {
            launch.environment_for_restore().into_iter().collect()
        } else {
            launch
                .environment
                .clone()
                .unwrap_or_else(|| std::env::vars_os().collect())
                .into_iter()
                .collect()
        };
        let shell = environment
            .get(std::ffi::OsStr::new("SHELL"))
            .cloned()
            .unwrap_or_else(|| {
                if account.shell.is_empty() {
                    os_constant(PROCESS_SHELL)
                } else {
                    account.shell
                }
            });
        for name in ["COLUMNS", "LINES", "TERMCAP"] {
            environment.remove(std::ffi::OsStr::new(name));
        }
        environment.insert("LOGNAME".into(), account.name.clone());
        environment.insert("USER".into(), account.name);
        environment.insert("HOME".into(), account.home);
        environment.insert("SHELL".into(), shell.clone());
        environment.insert("TERM".into(), os_constant(PROCESS_TERM_NAME));
        if !launch.window_id.is_empty() {
            environment.insert(
                "WINDOWID".into(),
                OsString::from_vec(launch.window_id.clone()),
            );
        }
        let (master, slave) = open_pty(launch.columns as u16, launch.rows as u16)?;
        let program = launch.arguments.first().cloned().unwrap_or(shell);
        let mut command = Command::new(program);
        command
            .args(launch.arguments.iter().skip(1))
            .current_dir(directory)
            .env_clear()
            .envs(environment)
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave));
        // This process is headless. Only async-signal-safe syscalls run between fork and exec.
        unsafe {
            command.pre_exec(|| {
                checked(libc::setsid())?;
                checked(libc::ioctl(libc::STDIN_FILENO, libc::TIOCSCTTY, 0))?;
                let mut signals: libc::sigset_t = mem::zeroed();
                libc::sigemptyset(&mut signals);
                checked(libc::sigprocmask(
                    libc::SIG_SETMASK,
                    &signals,
                    ptr::null_mut(),
                ))?;
                for signal in [
                    libc::SIGCHLD,
                    libc::SIGHUP,
                    libc::SIGINT,
                    libc::SIGQUIT,
                    libc::SIGTERM,
                    libc::SIGALRM,
                ] {
                    libc::signal(signal, libc::SIG_DFL);
                }
                Ok(())
            });
        }
        let child = match command.spawn() {
            Ok(child) => child,
            // C exec/cwd failures exit only the new child. Command has already reaped a failed exec;
            // closing its slave copies exposes EOF on this tab without killing unrelated sessions.
            Err(error)
                if matches!(
                    error.raw_os_error(),
                    Some(libc::ENOENT | libc::ENOTDIR | libc::EACCES | libc::ENOEXEC | libc::E2BIG)
                ) =>
            {
                return Ok(Self {
                    master: Some(master),
                    identity: Identity::None,
                    reaper,
                });
            }
            Err(error) => return Err(error),
        };
        Ok(Self {
            master: Some(master),
            identity: Identity::Local(child),
            reaper,
        })
    }

    pub fn master(&mut self) -> Option<&mut File> {
        self.master.as_mut()
    }
    pub fn fd(&self) -> Option<RawFd> {
        self.master.as_ref().map(AsRawFd::as_raw_fd)
    }

    pub fn live_pid(&mut self) -> libc::pid_t {
        match &mut self.identity {
            Identity::Local(child) => {
                if matches!(child.try_wait(), Ok(None)) {
                    child.id() as libc::pid_t
                } else {
                    0
                }
            }
            Identity::Transferred(identity) => identity.live_pid(),
            Identity::None => 0,
        }
    }

    pub fn resize(&self, columns: u16, rows: u16) -> io::Result<()> {
        if let Some(fd) = self.fd() {
            let size = libc::winsize {
                ws_col: columns,
                ws_row: rows,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            checked(unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &size) })?;
        }
        Ok(())
    }

    pub fn send_break(&self) -> io::Result<()> {
        if let Some(fd) = self.fd() {
            checked(unsafe { libc::tcsendbreak(fd, 0) })?;
        }
        Ok(())
    }

    pub fn stop(&mut self) {
        match mem::replace(&mut self.identity, Identity::None) {
            Identity::Local(mut child) => {
                // Child identity is valid only while this owner still holds its unreaped child.
                // Any wait error loses that authority; do not signal a potentially reused numeric PID.
                if matches!(child.try_wait(), Ok(None)) {
                    unsafe {
                        libc::kill(child.id() as libc::pid_t, libc::SIGHUP);
                    }
                }
                self.reaper.retain(child);
            }
            Identity::Transferred(identity) => {
                let _ = identity.signal(libc::SIGHUP);
            }
            Identity::None => {}
        }
        self.master.take();
    }
}

/// Prepared ownership never reads the master or signals the pinned process, including on drop.
pub(crate) struct PreparedPty {
    master: File,
    process: PinnedProcess,
}

impl PreparedPty {
    pub(crate) fn validate(master: File, process: PinnedProcess) -> io::Result<Self> {
        let flags = checked(unsafe { libc::fcntl(master.as_raw_fd(), libc::F_GETFL) })?;
        if unsafe { libc::isatty(master.as_raw_fd()) } != 1 || flags & libc::O_NONBLOCK == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "handoff master is not a nonblocking terminal",
            ));
        }
        checked(unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) })?;
        Ok(Self { master, process })
    }

    // All fallible validation is before the transaction's C marker; adoption itself cannot roll back.
    pub(crate) fn commit(self, reaper: ChildReaper) -> Pty {
        Pty {
            master: Some(self.master),
            identity: Identity::Transferred(self.process),
            reaper,
        }
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ChildGuard(Child);
    impl ChildGuard {
        fn new() -> Self {
            let mut command = Command::new("/bin/sleep");
            command
                .arg("30")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            unsafe {
                command.pre_exec(|| {
                    libc::signal(libc::SIGHUP, libc::SIG_DFL);
                    Ok(())
                });
            }
            Self(command.spawn().unwrap())
        }
        fn exited(&mut self) -> std::process::ExitStatus {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            loop {
                if let Some(status) = self.0.try_wait().unwrap() {
                    return status;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "owned fixture child did not exit"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            if matches!(self.0.try_wait(), Ok(None)) {
                let _ = self.0.kill();
            }
            let _ = self.0.wait();
        }
    }

    #[test]
    fn transferred_signals_use_the_pin_even_if_a_numeric_pid_is_reused() {
        let mut original = ChildGuard::new();
        let mut pinned = PinnedProcess::prepare(original.0.id() as libc::pid_t).unwrap();
        original.0.kill().unwrap();
        original.0.wait().unwrap();
        let mut unrelated = ChildGuard::new();
        // Deterministically model PID reuse without relying on the kernel to recycle a PID.
        pinned.pid = unrelated.0.id() as libc::pid_t;
        assert_eq!(pinned.live_pid(), 0);
        pinned.signal(libc::SIGHUP).unwrap();
        assert!(unrelated.0.try_wait().unwrap().is_none());
    }

    #[test]
    fn rejected_preparation_never_signals_the_donors_child() {
        let mut child = ChildGuard::new();
        let pinned = PinnedProcess::prepare(child.0.id() as libc::pid_t).unwrap();
        let (master, _slave) = open_pty(80, 24).unwrap();
        let prepared = PreparedPty::validate(master, pinned).unwrap();
        drop(prepared);
        assert!(child.0.try_wait().unwrap().is_none());
    }

    #[test]
    fn committed_processes_are_signalled_by_the_new_owner() {
        let mut child = ChildGuard::new();
        let pinned = PinnedProcess::prepare(child.0.id() as libc::pid_t).unwrap();
        let (master, _slave) = open_pty(80, 24).unwrap();
        let prepared = PreparedPty::validate(master, pinned).unwrap();
        let mut pty = prepared.commit(ChildReaper::default());
        pty.stop();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(child.exited().signal(), Some(libc::SIGHUP));
    }

    #[test]
    fn an_externally_reaped_child_loses_signal_authority() {
        let child = Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .unwrap();
        let pid = child.id() as libc::pid_t;
        // Model another reaper consuming the status. No reused PID is ever signalled by this test.
        checked(unsafe { libc::waitpid(pid, ptr::null_mut(), 0) }).unwrap();
        let (master, _slave) = open_pty(80, 24).unwrap();
        let mut pty = Pty {
            master: Some(master),
            identity: Identity::Local(child),
            reaper: ChildReaper::default(),
        };
        let live = pty.live_pid();
        pty.identity = Identity::None;
        assert_eq!(live, 0);
    }
}
