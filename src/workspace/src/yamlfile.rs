//! Plumbing shared by the per-user, hand-editable YAML files (device membership, bookmarks): a bounded read
//! that never follows links, and read-modify-publish under a non-blocking lock. A damaged file is reported,
//! never reset; the callers own the format.
use crate::statefile;
use std::{
    fs::{File, OpenOptions},
    io::{self, Read},
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::Path,
};

pub(crate) const FILE_LIMIT: usize = 65536;

/// The file's text, or `None` when it does not exist. `invalid` builds the caller's error for a bad file.
pub(crate) fn read(path: &Path, invalid: &dyn Fn(&str) -> io::Error) -> io::Result<Option<String>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path);
    let file = match file {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > FILE_LIMIT as u64 {
        return Err(invalid("not a regular file under 64 KiB"));
    }
    let mut bytes = Vec::new();
    file.take(FILE_LIMIT as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > FILE_LIMIT {
        return Err(invalid("larger than 64 KiB"));
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|error| invalid(&error.to_string()))
}

/// Run `change` while holding the file's lock and publish the bytes it returns (`None` leaves the file as is).
/// Scopes and windows share these per-user files, so the read-modify-publish is serialized across processes
/// without ever blocking an event loop: a busy lock fails with `WouldBlock` carrying `busy`.
pub(crate) fn update(
    path: &Path,
    invalid: &dyn Fn(&str) -> io::Error,
    busy: &str,
    change: impl FnOnce() -> io::Result<Option<Vec<u8>>>,
) -> io::Result<bool> {
    statefile::prepare_directory(path)?;
    let mut lock_path = path.as_os_str().to_os_string();
    lock_path.push(".lock");
    let lock: File = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(lock_path)?;
    if !lock.metadata()?.is_file() {
        return Err(invalid("lock is not a regular file"));
    }
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(io::Error::new(io::ErrorKind::WouldBlock, busy));
    }
    let Some(bytes) = change()? else {
        return Ok(false);
    };
    statefile::atomic_write(path, &bytes)?;
    Ok(true)
}
