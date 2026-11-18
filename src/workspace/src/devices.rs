//! Explicit per-user membership in a hand-editable YAML file. Reachability observations never
//! rewrite enrollment.
use crate::statefile;
use std::{
    fs::{File, OpenOptions},
    io::{self, Read},
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::PathBuf,
};
use worminal_protocol::{
    worp::devices::{Device, MAX_DEVICES},
};
use serde_yaml_ng::{Mapping, Value};

const FILE_LIMIT: usize = 65536;
const HEADER: &str = "# Worminal device membership: NAME: {endpoint: SSH_ENDPOINT}. Hand edits are fine;\n\
# `worminald api enroll/forget` rewrite this file without comments.\n";

pub struct Registry {
    path: PathBuf,
}

fn invalid(reason: impl std::fmt::Display) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("Invalid device registry ({reason}); fix or remove the file, it is never reset."),
    )
}

/// Hand-written YAML reads `node1: ...` keys as strings but `42: ...` as numbers; both are names.
fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn decode(text: &str) -> io::Result<Vec<Device>> {
    let root: Mapping = serde_yaml_ng::from_str::<Option<Mapping>>(text)
        .map_err(invalid)?
        .unwrap_or_default();
    let mut devices: Vec<Device> = Vec::new();
    for (key, value) in root {
        if key.as_str() != Some("devices") {
            return Err(invalid("only a top-level `devices` mapping is allowed"));
        }
        let entries = match value {
            Value::Null => Mapping::new(),
            Value::Mapping(entries) => entries,
            _ => return Err(invalid("`devices` must be a mapping")),
        };
        if entries.len() > MAX_DEVICES {
            return Err(invalid(format!("more than {MAX_DEVICES} devices")));
        }
        for (name, fields) in entries {
            let name = scalar(&name).ok_or_else(|| invalid("device names are scalars"))?;
            let Value::Mapping(fields) = fields else {
                return Err(invalid(format!("{name}: expected {{endpoint: ...}}")));
            };
            if fields.len() != 1 {
                return Err(invalid(format!("{name}: `endpoint` is the only field")));
            }
            let endpoint = fields
                .get("endpoint")
                .and_then(scalar)
                .ok_or_else(|| invalid(format!("{name}: `endpoint` must be a string")))?;
            let device = Device { name, endpoint };
            device
                .validate()
                .map_err(|error| invalid(format!("{}: {error}", device.name)))?;
            // YAML already rejects duplicate names; one route under two names is ambiguous.
            if devices.iter().any(|old| old.endpoint == device.endpoint) {
                return Err(invalid(format!("{}: endpoint enrolled twice", device.name)));
            }
            devices.push(device);
        }
    }
    devices.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(devices)
}

fn encode(devices: &[Device]) -> io::Result<Vec<u8>> {
    let entries: Mapping = devices
        .iter()
        .map(|device| {
            let mut fields = Mapping::new();
            fields.insert("endpoint".into(), device.endpoint.clone().into());
            (device.name.clone().into(), Value::Mapping(fields))
        })
        .collect();
    let mut root = Mapping::new();
    root.insert("devices".into(), Value::Mapping(entries));
    let body = serde_yaml_ng::to_string(&root).map_err(io::Error::other)?;
    Ok(format!("{HEADER}{body}").into_bytes())
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
            return Err(invalid("not a regular file under 64 KiB"));
        }
        let mut bytes = Vec::new();
        file.take(FILE_LIMIT as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > FILE_LIMIT {
            return Err(invalid("larger than 64 KiB"));
        }
        decode(std::str::from_utf8(&bytes).map_err(invalid)?)
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
            return Err(invalid("lock is not a regular file"));
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
