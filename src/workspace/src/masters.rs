//! Successful outbound view connections, with durable history and kernel-owned live leases.
use crate::statefile;
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::{Path, PathBuf},
};
use worminal_protocol::worp::devices::validate_endpoint;

#[derive(Debug, PartialEq, Eq)]
pub struct Master {
    pub endpoint: String,
    pub connected: bool,
}

pub struct Registry {
    directory: PathBuf,
}

/// Independent open-file-description locks survive unrelated closes and release on process death.
/// GETLK observes them without taking a lock: simultaneous listings cannot make each other look live.
pub struct Connection {
    _file: File,
}

fn lock(file: &File, command: libc::c_int, kind: libc::c_short) -> io::Result<libc::flock> {
    let mut record: libc::flock = unsafe { std::mem::zeroed() };
    record.l_type = kind;
    record.l_whence = libc::SEEK_SET as _;
    if unsafe { libc::fcntl(file.as_raw_fd(), command, &mut record) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(record)
}

fn open(path: &Path, create: bool) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(create)
        .create(create)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid master history record.",
        ));
    }
    Ok(file)
}

impl Registry {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    fn validate_root(&self) -> io::Result<()> {
        if !self.directory.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Master history needs an absolute state directory.",
            ));
        }
        Ok(())
    }

    pub fn connected(&self, endpoint: &str) -> io::Result<Connection> {
        self.validate_root()?;
        validate_endpoint(endpoint)?;
        // Valid SSH routes contain no '/' or '%'. Escape only the two directory navigation names;
        // using the route itself also keeps the maximum 255-byte endpoint within NAME_MAX.
        let name = if matches!(endpoint, "." | "..") {
            format!("%{endpoint}")
        } else {
            endpoint.to_owned()
        };
        let path = self.directory.join(name);
        statefile::prepare_directory(&path)?;
        let file = open(&path, true)?;
        lock(&file, libc::F_OFD_SETLK, libc::F_RDLCK as _)?;
        file.sync_all()?;
        File::open(&self.directory)?.sync_all()?;
        Ok(Connection { _file: file })
    }

    pub fn list(&self) -> io::Result<Vec<Master>> {
        self.validate_root()?;
        let entries = match fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut masters = Vec::new();
        for entry in entries {
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| io::Error::other("Invalid master history name."))?;
            let endpoint = match name.as_str() {
                "%." => ".",
                "%.." => "..",
                _ => &name,
            };
            validate_endpoint(endpoint)?;
            let file = open(&entry.path(), false)?;
            let observed = lock(&file, libc::F_OFD_GETLK, libc::F_WRLCK as _)?;
            masters.push(Master {
                endpoint: endpoint.to_owned(),
                connected: observed.l_type != libc::F_UNLCK as libc::c_short,
            });
        }
        masters.sort_by(|left, right| left.endpoint.cmp(&right.endpoint));
        Ok(masters)
    }
}
