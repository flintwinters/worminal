//! Local daemons start after display validation through the shared GUI-safe spawn boundary.
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
    let null = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/null")?;
    let arguments = [
        CString::new(path.as_os_str().as_bytes())?,
        CString::new("--serve").unwrap(),
    ];
    let pid = worminal_workspace::spawn::start(
        &arguments,
        &[(null.as_fd(), 0), (null.as_fd(), 1), (null.as_fd(), 2)],
        true,
    )?;
    let (sender, receiver) = mpsc::sync_channel(1);
    // Never kill a successful daemon spawn if waiter setup fails: it may already own restored shells.
    thread::Builder::new()
        .name("worminal-daemon-reap".into())
        .spawn(move || {
            let _ = sender.send(worminal_workspace::spawn::wait(pid));
        })?;
    Ok(receiver)
}
