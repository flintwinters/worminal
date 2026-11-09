//! Start SSH with posix_spawnp in its own session, so GTK fork hooks do not run and SSH cannot
//! stop the GUI process group through a terminal.
use std::{
    ffi::{CString, OsStr},
    io,
    os::{
        fd::AsFd,
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

fn connect_command(master: &OsStr, command: &[&[u8]]) -> io::Result<(UnixStream, Child)> {
    connect_program(master, WORKSPACE_DAEMON.to_bytes(), command)
}

/// Fixed bootstrap command; user data is sent on stdin, never interpolated into a remote shell.
pub fn connect_python(master: &OsStr, code: &str) -> io::Result<(UnixStream, Child)> {
    let quoted = format!("'{}'", code.replace('\'', "'\\''"));
    connect_program(master, b"python3", &[b"-c", quoted.as_bytes()])
}

fn connect_program(
    master: &OsStr,
    program: &[u8],
    command: &[&[u8]],
) -> io::Result<(UnixStream, Child)> {
    let bytes = master.as_bytes();
    if bytes.is_empty() || bytes[0] == b'-' || bytes.contains(&b'\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid master host",
        ));
    }
    let (parent, remote) = UnixStream::pair()?;
    let arguments = [
        b"ssh".as_slice(),
        b"-T",
        b"-C",
        b"-oConnectTimeout=10",
        bytes,
        program,
    ]
    .into_iter()
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
