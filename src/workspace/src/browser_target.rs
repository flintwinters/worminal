//! Resolve targets in the caller's directory before the service receives them.
use std::{
    ffi::{CStr, OsStr, OsString},
    fs, io,
    net::Ipv6Addr,
    os::unix::ffi::OsStrExt,
    path::Path,
};

pub const WORKSPACE_BROWSER_SCHEME: &str = "worminal";
pub const WORKSPACE_BROWSER_HOME: &str = "about:blank";

/// Native CLI conversion replaces this process with the service client before GUI initialization.
pub fn command(arguments: &[OsString]) -> io::Result<()> {
    if arguments
        .get(1)
        .is_none_or(|argument| argument != "--new-browser-tab")
    {
        return Ok(());
    }
    if arguments.len() != 3 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: worminal --new-browser-tab URL|FILE",
        ));
    }
    use std::{os::unix::process::CommandExt, process::Command};
    let daemon = CStr::from_bytes_with_nul(worminal_protocol::WORKSPACE_DAEMON).unwrap();
    let daemon = std::env::current_exe()?.with_file_name(OsStr::from_bytes(daemon.to_bytes()));
    Err(Command::new(daemon)
        .arg("--new-browser-tab")
        .arg(&arguments[2])
        .exec())
}

fn authority(bytes: &[u8]) -> bool {
    let end = bytes
        .iter()
        .position(|byte| b"/?#".contains(byte))
        .unwrap_or(bytes.len());
    let bytes = &bytes[..end];
    let host_end = if bytes.first() == Some(&b'[') {
        let Some(close) = bytes.iter().position(|byte| *byte == b']') else {
            return false;
        };
        if std::str::from_utf8(&bytes[1..close])
            .ok()
            .and_then(|value| value.parse::<Ipv6Addr>().ok())
            .is_none()
        {
            return false;
        }
        close + 1
    } else {
        let host_end = bytes
            .iter()
            .position(|byte| *byte == b':')
            .unwrap_or(bytes.len());
        let mut dotted = false;
        for (index, byte) in bytes[..host_end].iter().enumerate() {
            if *byte == b'.' {
                if index == 0 || b".-".contains(&bytes[index - 1]) {
                    return false;
                }
                dotted = true;
            } else if !(byte.is_ascii_alphanumeric()
                || (*byte == b'-' && index > 0 && bytes[index - 1] != b'.'))
            {
                return false;
            }
        }
        if host_end == 0
            || bytes[host_end - 1] == b'-'
            || (!dotted && !bytes[..host_end].eq_ignore_ascii_case(b"localhost"))
        {
            return false;
        }
        host_end
    };
    host_end == bytes.len()
        || (bytes[host_end] == b':'
            && host_end + 1 < bytes.len()
            && bytes[host_end + 1..].iter().all(u8::is_ascii_digit))
}

pub fn resolve(text: Option<&OsStr>) -> io::Result<Vec<u8>> {
    resolve_in(text, &std::env::current_dir()?)
}
pub fn resolve_in(text: Option<&OsStr>, directory: &Path) -> io::Result<Vec<u8>> {
    let Some(text) = text else {
        return Ok(WORKSPACE_BROWSER_HOME.as_bytes().to_vec());
    };
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let web = authority(bytes);
    if !web && bytes[0].is_ascii_alphabetic() {
        let end = bytes
            .iter()
            .skip(1)
            .position(|byte| !byte.is_ascii_alphanumeric() && !b"+-.".contains(byte))
            .map(|index| index + 1)
            .unwrap_or(bytes.len());
        if bytes.get(end) == Some(&b':') {
            return Ok(bytes.to_vec());
        }
    }
    let local = directory.join(text);
    let path = match fs::canonicalize(&local) {
        Ok(path) => path.as_os_str().as_bytes().to_vec(),
        Err(_) => {
            if web
                && fs::symlink_metadata(&local).is_err_and(|error| {
                    matches!(error.raw_os_error(), Some(libc::ENOENT | libc::ENOTDIR))
                })
            {
                let mut url = b"https://".to_vec();
                url.extend_from_slice(bytes);
                return Ok(url);
            }
            if bytes.starts_with(b"/") {
                bytes.to_vec()
            } else {
                let mut path = directory.as_os_str().as_bytes().to_vec();
                path.push(b'/');
                path.extend_from_slice(bytes);
                path
            }
        }
    };
    if path.len() > 65536 {
        return Err(io::Error::from_raw_os_error(libc::ENAMETOOLONG));
    }
    let mut url = b"file://".to_vec();
    const HEX: &[u8] = b"0123456789ABCDEF";
    for byte in path {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            url.push(byte);
        } else {
            url.extend_from_slice(&[b'%', HEX[(byte >> 4) as usize], HEX[(byte & 15) as usize]]);
        }
    }
    Ok(url)
}
