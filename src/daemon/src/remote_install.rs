//! Bootstrap a new headless node without copying credentials, project files or user state.
use serde_json::{json, Value};
use std::{
    cmp::Ordering,
    ffi::OsString,
    fs::File,
    io::{self, BufReader, Read, Write},
    os::unix::net::UnixStream,
    process::{Command, Stdio},
    time::Duration,
};
use worminal_protocol::worp::devices::validate_endpoint;
use worminal_workspace::ssh;

const LAYOUT: &str = include_str!("../../../assets/headless-install.json");
const BOOTSTRAP: &str = include_str!("../../../tools/remote_install.py");
const MANUAL: &str = include_str!("../../../docs/man/worminal.1");
const LIMIT: u64 = 64 * 1024 * 1024;
const HEADER_LIMIT: usize = 16384;
const UPGRADE_HELP: &str = "Usage: worminald upgrade-remote SSH_ENDPOINT\n\
Make this node and an installed peer run the same worminald, whichever is older.\n\
Compares Cargo versions; a newer local daemon is pushed to the peer, a newer peer daemon is pulled here.\n\
Both sides must be Linux on the same CPU, with Python 3 and non-interactive sudo, installed in /usr/local.\n\
Files are replaced atomically; a running daemon hands its shells to the new file without restarting them.\n\
Active v1 workspaces are never touched. Views run this automatically once per host when they connect.\n";
const HELP: &str = "Usage: worminald install-on-remote SSH_ENDPOINT\n\
Install this headless daemon and its manual on a new compatible Linux node over SSH.\n\
Requires Python 3 and non-interactive sudo on the target. Installs into /usr/local.\n\
Existing installations/workspaces are refused; no daemon is started and no device is enrolled.\n\
Only matching CPU architectures are supported; the staged binary must also run on the target.\n\
No credentials, source checkout, configuration or user state are transferred.\n";

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}

fn receive(reader: &mut impl Read) -> io::Result<Value> {
    let mut bytes = Vec::new();
    for _ in 0..HEADER_LIMIT {
        let mut byte = [0];
        reader.read_exact(&mut byte).map_err(|_| {
            invalid("SSH bootstrap did not respond; verify SSH access and Python 3 on the target.")
        })?;
        bytes.push(byte[0]);
        if byte[0] == b'\n' {
            let value: Value = serde_json::from_slice(&bytes)
                .map_err(|_| invalid("Invalid installer response."))?;
            if value.get("ok").and_then(Value::as_bool) == Some(false) {
                return Err(invalid(
                    value
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("Remote install failed."),
                ));
            }
            return Ok(value);
        }
    }
    Err(invalid("Remote installation response exceeds the limit."))
}

fn check_platform(value: &Value) -> io::Result<()> {
    let machine = std::env::consts::ARCH;
    if value.get("ready").and_then(Value::as_bool) != Some(true)
        || value.get("system").and_then(Value::as_str) != Some("Linux")
        || value.get("machine").and_then(Value::as_str) != Some(machine)
    {
        return Err(invalid(format!(
            "Target must be Linux/{machine}, matching this daemon. Nothing was installed."
        )));
    }
    Ok(())
}

fn code(mode: &str) -> String {
    format!(
        "import json\nLAYOUT=json.loads({})\nMODE={}\nSOURCE={}\nexec(SOURCE)\n",
        serde_json::to_string(LAYOUT).unwrap(),
        serde_json::to_string(mode).unwrap(),
        serde_json::to_string(BOOTSTRAP).unwrap()
    )
}

type Version = (u64, u64, u64);
fn parse(text: &str) -> Option<Version> {
    let mut parts = text.split_whitespace().last()?.split('.');
    let mut next = || parts.next()?.parse().ok();
    Some((next()?, next()?, next()?))
}
/// Order of a node's installed version against `ours`; a daemon predating `--version` is older.
fn compare(theirs: Option<&str>, ours: &str) -> io::Result<Ordering> {
    let ours = parse(ours).ok_or_else(|| invalid("Invalid compiled version."))?;
    match theirs {
        None => Ok(Ordering::Less),
        Some(text) => parse(text)
            .map(|version| version.cmp(&ours))
            .ok_or_else(|| invalid("Unrecognized peer version; nothing was replaced.")),
    }
}

fn install(endpoint: &str) -> io::Result<Value> {
    validate_endpoint(endpoint)?;
    let (mut stream, _child) = ssh::connect_python(endpoint.as_ref(), &code("install"))?;
    stream.set_read_timeout(Some(Duration::from_secs(120)))?;
    stream.set_write_timeout(Some(Duration::from_secs(120)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    check_platform(&receive(&mut reader)?)?;
    let mut result = push(&mut stream, &mut reader)?;
    result["endpoint"] = json!(endpoint);
    result["next"] = json!(format!("Open a view with: worminal --master {endpoint}"));
    Ok(result)
}

/// Send this running daemon and manual to a peer whose bootstrap is waiting for the payload header.
fn push(stream: &mut UnixStream, reader: &mut BufReader<UnixStream>) -> io::Result<Value> {
    // Pin the actual running inode, including when its installed pathname has been replaced.
    let binary = File::open("/proc/self/exe")?;
    let size = binary.metadata()?.len();
    if size == 0 || size > LIMIT {
        return Err(invalid(
            "Daemon exceeds the 64 MiB limit; use a release build.",
        ));
    }
    let manual = MANUAL
        .replace("VERSION", env!("CARGO_PKG_VERSION"))
        .into_bytes();
    let layout: Value = serde_json::from_str(LAYOUT)
        .map_err(|_| invalid("Invalid compiled installation layout."))?;
    let files = layout["files"]
        .as_array()
        .ok_or_else(|| invalid("Invalid compiled installation layout."))?;
    let records: Vec<_> = files
        .iter()
        .map(|file| {
            let role = file["role"].as_str().unwrap_or("");
            json!({"role":role,"size":if role == "daemon" { size } else { manual.len() as u64 }})
        })
        .collect();
    writeln!(stream, "{}", json!({"files":records}))?;
    stream.flush()?;
    if receive(reader)?.get("accept").and_then(Value::as_bool) != Some(true) {
        return Err(invalid("Target did not accept the installation payload."));
    }
    for file in files {
        match file["role"].as_str() {
            Some("daemon") => {
                io::copy(&mut binary.try_clone()?.take(size), &mut *stream)?;
            }
            Some("manual") => stream.write_all(&manual)?,
            _ => return Err(invalid("Unknown compiled installation resource.")),
        }
    }
    stream.flush()?;
    let result = receive(reader)?;
    if result.get("installed").and_then(Value::as_bool) != Some(true) {
        return Err(invalid(
            "Target did not confirm installation; no automatic retry was made.",
        ));
    }
    Ok(result)
}

fn timeouts(stream: &UnixStream) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(120)))?;
    stream.set_write_timeout(Some(Duration::from_secs(120)))
}

/// Bring whichever of this node and `endpoint` runs the older daemon up to the newer one.
fn upgrade(endpoint: &str) -> io::Result<Value> {
    validate_endpoint(endpoint)?;
    let (mut stream, _child) = ssh::connect_python(endpoint.as_ref(), &code("upgrade"))?;
    timeouts(&stream)?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let ready = receive(&mut reader)?;
    check_platform(&ready)?;
    let theirs = ready.get("version").and_then(Value::as_str);
    let mut result = match compare(theirs, env!("CARGO_PKG_VERSION"))? {
        Ordering::Less => {
            let mut result = push(&mut stream, &mut reader)?;
            result["direction"] = json!("push");
            result
        }
        Ordering::Equal => finish_current(&mut stream, &mut reader)?,
        Ordering::Greater => pull(&mut stream, &mut reader, theirs.unwrap_or(""))?,
    };
    result["endpoint"] = json!(endpoint);
    Ok(result)
}

fn finish_current(
    stream: &mut UnixStream,
    reader: &mut BufReader<UnixStream>,
) -> io::Result<Value> {
    writeln!(stream, "{}", json!({"cancel":true}))?;
    stream.flush()?;
    receive(reader)
}

/// The peer is newer: run the same bootstrap here as the target and feed it the peer's files.
fn pull(
    stream: &mut UnixStream,
    reader: &mut BufReader<UnixStream>,
    theirs: &str,
) -> io::Result<Value> {
    let mut local = Command::new("python3")
        .args(["-c", &code("upgrade")])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let result = (|| {
        let mut input = local
            .stdin
            .take()
            .ok_or_else(|| invalid("No local bootstrap."))?;
        let mut output = BufReader::new(
            local
                .stdout
                .take()
                .ok_or_else(|| invalid("No local bootstrap."))?,
        );
        let ours = receive(&mut output)?;
        if compare(ours.get("version").and_then(Value::as_str), theirs)? != Ordering::Less {
            writeln!(input, "{}", json!({"cancel":true}))?;
            input.flush()?;
            receive(&mut output)?;
            return finish_current(stream, reader);
        }
        writeln!(stream, "{}", json!({"send":true}))?;
        stream.flush()?;
        let header = receive(reader)?;
        let records = header
            .get("files")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("Invalid peer payload header."))?;
        let sizes = records
            .iter()
            .map(|record| record.get("size").and_then(Value::as_u64))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| invalid("Invalid peer payload header."))?;
        writeln!(input, "{}", json!({"files":records}))?;
        input.flush()?;
        if receive(&mut output)?.get("accept").and_then(Value::as_bool) != Some(true) {
            return Err(invalid("This node did not accept the peer payload."));
        }
        for size in sizes {
            if size == 0 || size > LIMIT {
                return Err(invalid("Peer payload exceeds the size limit."));
            }
            if io::copy(&mut reader.by_ref().take(size), &mut input)? != size {
                return Err(invalid("Peer transfer interrupted; nothing was replaced."));
            }
        }
        drop(input);
        receive(reader)?;
        let mut result = receive(&mut output)?;
        if result.get("installed").and_then(Value::as_bool) != Some(true) {
            return Err(invalid("This node did not confirm the upgrade."));
        }
        result["direction"] = json!("pull");
        Ok(result)
    })();
    if result.is_err() {
        let _ = local.kill();
    }
    let _ = local.wait();
    result
}

pub fn run(arguments: &[OsString], upgrading: bool) -> io::Result<i32> {
    if arguments.len() == 1 && matches!(arguments[0].to_str(), Some("--help" | "-h" | "help")) {
        print!("{}", if upgrading { UPGRADE_HELP } else { HELP });
        return Ok(0);
    }
    let result = match arguments {
        [endpoint] => endpoint
            .to_str()
            .ok_or_else(|| invalid("SSH endpoint must be UTF-8."))
            .and_then(|endpoint| {
                if upgrading {
                    upgrade(endpoint)
                } else {
                    install(endpoint)
                }
            }),
        _ => Err(invalid("Run worminald install-on-remote --help for usage.")),
    };
    let result = match result {
        Ok(result) => result,
        Err(error) => json!({"ok":false,"error":error.to_string()}),
    };
    if upgrading && result.get("current").is_none() {
        // Detached views discard stdout, so record changes and failures where daemon events go.
        crate::process::diagnostic(&format!("Remote upgrade: {result}"));
    }
    println!("{result}");
    if upgrading && result.get("ok").and_then(Value::as_bool) != Some(false) {
        // Settings follow versions: once both sides run compatible code, merge their worminal.yaml.
        if let [endpoint] = arguments {
            let report = endpoint
                .to_str()
                .ok_or_else(|| invalid("SSH endpoint must be UTF-8."))
                .and_then(|endpoint| {
                    crate::config_sync::run(endpoint, worminal_config::sync::Prefer::Neither)
                })
                .unwrap_or_else(|error| json!({"ok": false, "error": error.to_string()}));
            crate::process::diagnostic(&format!("Config sync: {report}"));
            println!("{report}");
        }
    }
    Ok(
        if result.get("ok").and_then(Value::as_bool) == Some(false) {
            1
        } else {
            0
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_newer_and_unversioned_peers_are_ordered_against_us() {
        assert_eq!(compare(None, "0.9.3").unwrap(), Ordering::Less);
        assert_eq!(
            compare(Some("worminald 0.9.2"), "0.9.3").unwrap(),
            Ordering::Less
        );
        assert_eq!(
            compare(Some("worminald 0.9.3"), "0.9.3").unwrap(),
            Ordering::Equal
        );
        assert_eq!(
            compare(Some("worminald 0.10.0"), "0.9.3").unwrap(),
            Ordering::Greater
        );
        assert!(compare(Some("garbage"), "0.9.3").is_err());
    }
}
