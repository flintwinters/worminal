//! Start SSH with posix_spawnp in its own session, so GTK fork hooks do not run and SSH cannot
//! stop the GUI process group through a terminal.
use std::{
    ffi::{CString, OsStr},
    io,
    os::{
        fd::{AsFd, AsRawFd},
        unix::{ffi::OsStrExt, net::UnixStream},
    },
};
use worminal_protocol::WORKSPACE_DAEMON;

pub use crate::spawn::Child;

pub fn connect(master: &OsStr) -> io::Result<(UnixStream, Child)> {
    connect_command(master, &[b"--bridge"])
}

/// Replacement attaches only to an existing remote service; it never restores shells.
pub fn connect_existing(master: &OsStr) -> io::Result<(UnixStream, Child)> {
    connect_command(master, &[b"--bridge-existing"])
}

/// Worp inspection never starts a remote workspace as a side effect.
pub fn connect_worp(master: &OsStr) -> io::Result<(UnixStream, Child)> {
    connect_command(master, &[b"api", b"--stdio"])
}

/// Config sync talks to `worminald config exchange`; it never starts or restores a workspace.
pub fn connect_config(master: &OsStr) -> io::Result<(UnixStream, Child)> {
    connect_command(master, &[b"config", b"exchange"])
}

fn connect_command(master: &OsStr, command: &[&[u8]]) -> io::Result<(UnixStream, Child)> {
    connect_program(master, WORKSPACE_DAEMON.to_bytes(), command)
}

/// Fixed bootstrap command; user data is sent on stdin, never interpolated into a remote shell.
pub fn connect_python(master: &OsStr, code: &str) -> io::Result<(UnixStream, Child)> {
    connect_python_with_guard(master, code, None)
}

/// A proxy owns this SSH child; its death must also close the authenticated remote connection.
pub fn connect_python_guarded(
    master: &OsStr,
    code: &str,
    guard: &OsStr,
) -> io::Result<(UnixStream, Child)> {
    connect_python_with_guard(master, code, Some(guard))
}

fn connect_python_with_guard(
    master: &OsStr,
    code: &str,
    guard: Option<&OsStr>,
) -> io::Result<(UnixStream, Child)> {
    let quoted = format!("'{}'", code.replace('\'', "'\\''"));
    connect_program_with_guard(master, b"python3", &[b"-c", quoted.as_bytes()], guard)
}

/// `worminald --connect-master` writes a failed version sync to this descriptor, when present,
/// so the view can tell the user why the link closed. The helper owns the authoritative name.
pub const CONNECT_REPORT_FD: i32 = 3;
/// Opens a report about a peer that never answered. That is a network condition the status bar
/// shows while retrying, not a version-sync failure that deserves a desktop notification.
pub const UNREACHABLE_REPORT: &str = "Cannot reach";
pub const CONNECT_REPORT_ENV: &str = "WORMINAL_CONNECT_REPORT_FD";

/// GUI connections supply their own prompt helper; headless callers keep normal SSH policy.
/// The returned non-blocking pipe carries the helper's failure report (see CONNECT_REPORT_FD).
/// The GUI executable a daemon started by a window uses as the SSH askpass of its master links. Windows
/// run under several names (installed, preview, development builds), so the daemon cannot derive it.
pub const GUI_ENV: &str = "WORMINAL_GUI";

pub fn connect_gui(
    master: &OsStr,
    existing: bool,
    askpass: &OsStr,
    interactive: bool,
) -> io::Result<(UnixStream, Child, io::PipeReader)> {
    let daemon = std::path::Path::new(askpass).with_file_name("worminald");
    let (parent, remote) = UnixStream::pair()?;
    let (report, report_writer) = io::pipe()?;
    let report_fd = CONNECT_REPORT_FD.to_string();
    // The GUI reads the report only after the link closes; it must never block there.
    if unsafe { libc::fcntl(report.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let arguments = [
        CString::new(daemon.as_os_str().as_bytes())?,
        CString::new("--connect-master")?,
        CString::new(master.as_bytes())?,
        CString::new(if existing {
            "--bridge-existing"
        } else {
            "--bridge"
        })?,
    ];
    let pid = crate::spawn::start_with_environment(
        &arguments,
        &[
            (remote.as_fd(), 0),
            (remote.as_fd(), 1),
            (report_writer.as_fd(), CONNECT_REPORT_FD),
        ],
        true,
        &[
            (CONNECT_REPORT_ENV, OsStr::new(&report_fd)),
            ("SSH_ASKPASS", askpass),
            (
                "SSH_ASKPASS_REQUIRE",
                OsStr::new(if interactive { "force" } else { "never" }),
            ),
            ("WORMINAL_SSH_ASKPASS", OsStr::new("1")),
        ],
    )?;
    Ok((
        parent,
        crate::spawn::own(pid, "worminal-master-reap")?,
        report,
    ))
}

/// OpenSSH invokes the helper with a single prompt argument, outside the GUI broker.
pub fn is_askpass(arguments: &[std::ffi::OsString]) -> bool {
    arguments.len() == 2
        && std::env::var_os("WORMINAL_SSH_ASKPASS").as_deref() == Some(OsStr::new("1"))
}

fn connect_program(
    master: &OsStr,
    program: &[u8],
    command: &[&[u8]],
) -> io::Result<(UnixStream, Child)> {
    connect_program_with_guard(master, program, command, None)
}

fn connect_program_with_guard(
    master: &OsStr,
    program: &[u8],
    command: &[&[u8]],
    guard: Option<&OsStr>,
) -> io::Result<(UnixStream, Child)> {
    let bytes = master.as_bytes();
    if bytes.is_empty() || bytes[0] == b'-' || bytes.contains(&b'\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid master host",
        ));
    }
    let (parent, remote) = UnixStream::pair()?;
    let owner_pid = std::process::id().to_string();
    let prefix = guard.map(|guard| [guard.as_bytes(), b"--ssh-child", owner_pid.as_bytes()]);
    let arguments = prefix
        .into_iter()
        .flatten()
        .chain([
            b"ssh".as_slice(),
            b"-T",
            b"-C",
            b"-oConnectTimeout=10",
            bytes,
            program,
        ])
        .chain(command.iter().copied())
        .map(CString::new)
        .collect::<Result<Vec<_>, _>>()?;
    // SSH leads its own session, with no controlling terminal. The GUI owner inherits the
    // desktop's terminal and process group (e.g. startx on tty1 shares one group with the window
    // manager). An SSH host-key or password prompt then opened /dev/tty from a background group,
    // and the kernel stopped that whole group with SIGTTOU/SIGTTIN, freezing the desktop.
    // Detached, SSH cannot prompt on a terminal: it fails, or uses a configured SSH_ASKPASS.
    let pid = crate::spawn::start(
        &arguments,
        &[(remote.as_fd(), 0), (remote.as_fd(), 1)],
        true,
    )?;
    let child = crate::spawn::own(pid, "worminal-ssh-reap")?;
    Ok((parent, child))
}
