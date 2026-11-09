//! Explicit per-user membership. Reachability observations never rewrite enrollment.
use crate::statefile;
use std::{
    fs::{File, OpenOptions},
    io::{self, Read},
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::PathBuf,
};
use worminal_protocol::{
    worp::devices::{Device, MAX_DEVICES},
    Buffer, Payload,
};

const VERSION: u32 = 1;
const FILE_LIMIT: usize = 65536;

pub struct Registry {
    path: PathBuf,
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "Invalid device registry; preserve the original file.",
    )
}

fn decode(bytes: &[u8]) -> io::Result<Vec<Device>> {
    let mut payload = Payload::new(bytes);
    if payload.u32()? != VERSION {
        return Err(invalid());
    }
    let count = payload.u32()? as usize;
    if count > MAX_DEVICES {
        return Err(invalid());
    }
    let mut devices: Vec<Device> = Vec::with_capacity(count);
    for _ in 0..count {
        let device = Device {
            name: std::str::from_utf8(payload.string()?)
                .map_err(|_| invalid())?
                .to_owned(),
            endpoint: std::str::from_utf8(payload.string()?)
                .map_err(|_| invalid())?
                .to_owned(),
        };
        device.validate().map_err(|_| invalid())?;
        if devices
            .iter()
            .any(|old| old.name == device.name || old.endpoint == device.endpoint)
        {
            return Err(invalid());
        }
        devices.push(device);
    }
    payload.finish()?;
    Ok(devices)
}

fn encode(devices: &[Device]) -> io::Result<Vec<u8>> {
    let mut bytes = Buffer::with_limit(FILE_LIMIT);
    bytes.put_u32(VERSION)?;
    bytes.put_u32(devices.len() as u32)?;
    for device in devices {
        bytes.put_string(device.name.as_bytes())?;
        bytes.put_string(device.endpoint.as_bytes())?;
    }
    Ok(bytes.into_bytes())
}

impl Registry {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn list(&self) -> io::Result<Vec<Device>> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&self.path);
        let file = match file {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > FILE_LIMIT as u64 {
            return Err(invalid());
        }
        let mut bytes = Vec::new();
        file.take(FILE_LIMIT as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > FILE_LIMIT {
            return Err(invalid());
        }
        decode(&bytes).map_err(|_| invalid())
    }

    fn update(
        &self,
        change: impl FnOnce(&mut Vec<Device>) -> io::Result<bool>,
    ) -> io::Result<bool> {
        statefile::prepare_directory(&self.path)?;
        let mut lock_path = self.path.as_os_str().to_os_string();
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
            return Err(invalid());
        }
        // Scopes share this per-user registry. Serialize read-modify-publish, including
        // separate daemon processes, without blocking any terminal event loop on a lock.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Registry is busy; try again.",
            ));
        }
        let mut devices = self.list()?;
        let changed = change(&mut devices)?;
        if changed {
            statefile::atomic_write(&self.path, &encode(&devices)?)?;
        }
        Ok(changed)
    }

    pub fn enroll(&self, device: Device) -> io::Result<bool> {
        device.validate()?;
        self.update(|devices| {
            if let Some(old) = devices.iter().find(|old| old.name == device.name) {
                return if old == &device {
                    Ok(false)
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "Name already has another endpoint; \
                         forget it explicitly before re-enrolling.",
                    ))
                };
            }
            if devices.iter().any(|old| old.endpoint == device.endpoint) {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "Endpoint is already enrolled under another name.",
                ));
            }
            if devices.len() >= MAX_DEVICES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("Device registry is full ({MAX_DEVICES} devices)."),
                ));
            }
            devices.push(device);
            devices.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(true)
        })
    }

    pub fn forget(&self, name: &str) -> io::Result<bool> {
        worminal_protocol::worp::devices::validate_name(name)?;
        self.update(|devices| {
            let before = devices.len();
            devices.retain(|device| device.name != name);
            Ok(before != devices.len())
        })
    }
}
