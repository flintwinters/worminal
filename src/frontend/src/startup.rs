//! Detached helpers (the local daemon, link openers) start through the shared GUI-safe spawn
//! boundary: own session, /dev/null stdio, and a reaper thread.
use std::{
    ffi::CString,
    fs::OpenOptions,
    io,
    os::{fd::AsFd, unix::ffi::OsStrExt},
    path::Path,
    sync::mpsc::{self, Receiver},
    thread,
};

pub fn daemon(path: &Path) -> io::Result<Receiver<io::Result<i32>>> {
    detached(&[
        CString::new(path.as_os_str().as_bytes())?,
        CString::new("--serve").unwrap(),
    ])
}
pub fn open_url(url: &str) -> io::Result<()> {
    detached(&[CString::new("xdg-open").unwrap(), CString::new(url)?]).map(drop)
}
fn detached(arguments: &[CString]) -> io::Result<Receiver<io::Result<i32>>> {
    let null = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/null")?;
    let pid = worminal_workspace::spawn::start(
        arguments,
        &[(null.as_fd(), 0), (null.as_fd(), 1), (null.as_fd(), 2)],
        true,
    )?;
    let (sender, receiver) = mpsc::sync_channel(1);
    // Never kill a successful spawn if waiter setup fails: a daemon may already own restored shells.
    thread::Builder::new()
        .name("worminal-reap".into())
        .spawn(move || {
            let _ = sender.send(worminal_workspace::spawn::wait(pid));
        })?;
    Ok(receiver)
}
