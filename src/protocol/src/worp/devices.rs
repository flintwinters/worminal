//! Enrollment identifies user-chosen SSH routes, not physical machines.
use std::io;

pub const MAX_DEVICES: usize = 32;
const NAME_BYTES: usize = 64;
const ENDPOINT_BYTES: usize = 255;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub name: String,
    pub endpoint: String,
}

pub fn validate_name(name: &str) -> io::Result<()> {
    if name.is_empty()
        || name.len() > NAME_BYTES
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-".contains(&byte))
        || !name.as_bytes()[0].is_ascii_alphanumeric()
    {
        return Err(io::Error::new(io::ErrorKind::InvalidInput,
            "Device name must start with a letter/digit and contain 1-64 lowercase letters, digits, '-' or '_'."));
    }
    Ok(())
}

impl Device {
    pub fn validate(&self) -> io::Result<()> {
        validate_name(&self.name)?;
        validate_endpoint(&self.endpoint)?;
        Ok(())
    }
}

pub fn validate_endpoint(endpoint: &str) -> io::Result<()> {
    let bytes = endpoint.as_bytes();
    if bytes.is_empty()
        || bytes.len() > ENDPOINT_BYTES
        || bytes[0] == b'-'
        || endpoint.split('@').count() > 2
        || endpoint.split('@').any(str::is_empty)
        || endpoint
            .split_once('@')
            .is_some_and(|(user, _)| user.contains(':'))
        || !bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-@[]:+".contains(byte))
    {
        return Err(io::Error::new(io::ErrorKind::InvalidInput,
                "Endpoint must be a host or user@host SSH destination (at most 255 bytes), not a URL or command."));
    }
    Ok(())
}
