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
use worminal_workspace::{cli_output, cli_schema::Api, ssh};

const LAYOUT: &str = include_str!("../../../assets/headless-install.json");
const BOOTSTRAP: &str = include_str!("../../../tools/remote_install.py");
const MANUAL: &str = include_str!("../../../docs/man/worminal.1");
const DESKTOP_ENTRY: &str = include_str!("../../../assets/worminal.desktop");
const ICON: &str = include_str!("../../../assets/worminal.svg");
const FRONTEND_HEADER: &str = include_str!("../../gtk/rust_frontend_module.h");
const BROWSER_HEADER: &str = include_str!("../../gtk/rust_browser_module.h");
const LIMIT: u64 = 64 * 1024 * 1024;
const HEADER_LIMIT: usize = 16384;
pub(crate) const UPGRADE_HELP: &str = "Usage: worminald upgrade-remote SSH_ENDPOINT\n\
Make this node and an installed peer run the same worminald, whichever is older.\n\
Compares Cargo versions; a newer local daemon is pushed to the peer, a newer peer daemon is pulled here.\n\
Both sides must be Linux on the same CPU, with Python 3 and non-interactive sudo, installed in /usr/local.\n\
Files are replaced atomically; a running daemon hands its shells to the new file without restarting them.\n\
Active v1 workspaces are never touched. Views update immediately after SSH authentication, before opening the bridge.\n";
pub(crate) const HELP: &str = "Usage: worminald install-on-remote SSH_ENDPOINT\n\
Install this daemon and its manual on a new compatible Linux node over SSH.\n\
A target with GTK/WebKit also gets the worminal command, modules, desktop entry and icon.\n\
Requires Python 3 and non-interactive sudo on the target. Installs into /usr/local.\n\
Existing installations/workspaces are refused; no daemon is started and no device is enrolled.\n\
Only matching CPU architectures are supported; the staged binary must also run on the target.\n\
No credentials, source checkout, configuration or user state are transferred.\n";

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}

pub(super) fn receive(reader: &mut impl Read) -> io::Result<Value> {
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

/// The value a header names, e.g. the module file in `#define WORMINAL_RUST_FRONTEND_FILE "name.so"`.
fn defined(header: &str, name: &str) -> Option<String> {
    let line = header
        .lines()
        .find(|line| line.starts_with(&format!("#define {name} ")))?;
    Some(line.split('"').nth(1)?.to_string())
}

/// The graphical files in publication order (modules before the launcher that loads them), keyed by role.
fn desktop_layout() -> Vec<Value> {
    let module = |header, name| defined(header, name).map(|file| format!("lib/worminal/{file}"));
    let (Some(frontend), Some(browser)) = (
        module(FRONTEND_HEADER, "WORMINAL_RUST_FRONTEND_FILE"),
        module(BROWSER_HEADER, "WORMINAL_RUST_BROWSER_FILE"),
    ) else {
        return Vec::new();
    };
    let entry = |role: &str, path: &str, mode: u32| json!({"role": role, "base": "prefix", "path": path, "mode": mode});
    vec![
        entry("browser", &browser, 0o755),
        entry("frontend", &frontend, 0o755),
        entry("launcher", "bin/worminal", 0o755),
        entry("desktop", "share/applications/worminal.desktop", 0o644),
        entry(
            "icon",
            "share/icons/hicolor/scalable/apps/worminal.svg",
            0o644,
        ),
    ]
}

fn layout() -> Value {
    let mut layout: Value = serde_json::from_str(LAYOUT).expect("compiled installation layout");
    layout["desktop"] = Value::Array(desktop_layout());
    layout
}

pub(super) fn code(mode: &str) -> String {
    format!(
        "import json\nLAYOUT=json.loads({})\nMODE={}\nSOURCE={}\nexec(SOURCE)\n",
        serde_json::to_string(&layout().to_string()).unwrap(),
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
    let ready = receive(&mut reader)?;
    check_platform(&ready)?;
    let mut result = push(&mut stream, &mut reader, &ready)?;
    result["endpoint"] = json!(endpoint);
    result["next"] = json!(format!("Open a view with: worminal --master {endpoint}"));
    Ok(result)
}

/// One payload file: the running daemon streams from its pinned inode; everything else is already in memory.
enum Body {
    Running(File, u64),
    Bytes(Vec<u8>),
}
impl Body {
    fn size(&self) -> u64 {
        match self {
            Body::Running(_, size) => *size,
            Body::Bytes(bytes) => bytes.len() as u64,
        }
    }
    fn send(&self, stream: &mut UnixStream) -> io::Result<()> {
        match self {
            Body::Running(file, size) => {
                io::copy(&mut file.try_clone()?.take(*size), stream).map(drop)
            }
            Body::Bytes(bytes) => stream.write_all(bytes),
        }
    }
}

/// The installed graphical command beside the running daemon (`PREFIX/bin/worminald`), if this node has one.
/// Only the launcher and its two modules come from disk; the entry and icon are compiled in.
fn desktop_bodies(layout: &[Value]) -> Option<Vec<(String, Body)>> {
    let exe = std::fs::read_link("/proc/self/exe").ok()?;
    let exe = exe.to_str()?.trim_end_matches(" (deleted)").to_string();
    let prefix = std::path::Path::new(&exe).parent()?.parent()?.to_path_buf();
    if std::path::Path::new(&exe).parent()?.file_name()? != "bin" {
        return None;
    }
    layout
        .iter()
        .map(|file| {
            let role = file["role"].as_str()?;
            let body = match role {
                "desktop" => Body::Bytes(DESKTOP_ENTRY.as_bytes().to_vec()),
                "icon" => Body::Bytes(ICON.as_bytes().to_vec()),
                _ => {
                    let mut bytes = Vec::new();
                    File::open(prefix.join(file["path"].as_str()?))
                        .ok()?
                        .take(LIMIT + 1)
                        .read_to_end(&mut bytes)
                        .ok()?;
                    if bytes.is_empty() || bytes.len() as u64 > LIMIT {
                        return None;
                    }
                    Body::Bytes(bytes)
                }
            };
            Some((role.to_string(), body))
        })
        .collect()
}

/// Send this running daemon and manual to a peer whose bootstrap is waiting for the payload header.
/// A peer that can run graphics and has no `worminal` command also gets ours, when this node has one.
fn push(
    stream: &mut UnixStream,
    reader: &mut BufReader<UnixStream>,
    ready: &Value,
) -> io::Result<Value> {
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
    let layout = layout();
    let required = layout["files"]
        .as_array()
        .ok_or_else(|| invalid("Invalid compiled installation layout."))?;
    let mut bodies = Vec::new();
    for file in required {
        bodies.push(match file["role"].as_str() {
            Some("daemon") => (
                "daemon".to_string(),
                Body::Running(binary.try_clone()?, size),
            ),
            Some("manual") => ("manual".to_string(), Body::Bytes(manual.clone())),
            _ => return Err(invalid("Unknown compiled installation resource.")),
        });
    }
    let wants = |key| ready.get(key).and_then(Value::as_bool);
    if wants("graphics") == Some(true) && wants("desktop") == Some(false) {
        if let Some(desktop) = layout["desktop"]
            .as_array()
            .and_then(|layout| desktop_bodies(layout))
        {
            bodies.extend(desktop);
        }
    }
    let records: Vec<_> = bodies
        .iter()
        .map(|(role, body)| json!({"role": role, "size": body.size()}))
        .collect();
    writeln!(stream, "{}", json!({"files":records}))?;
    stream.flush()?;
    if receive(reader)?.get("accept").and_then(Value::as_bool) != Some(true) {
        return Err(invalid("Target did not accept the installation payload."));
    }
    for (_, body) in &bodies {
        body.send(stream)?;
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
    let mut result = upgrade_stream(&mut stream)?;
    result["endpoint"] = json!(endpoint);
    Ok(result)
}

/// Version negotiation is shared by the explicit command and the authenticated view gate.
pub(super) fn upgrade_stream(stream: &mut UnixStream) -> io::Result<Value> {
    timeouts(stream)?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let ready = receive(&mut reader)?;
    check_platform(&ready)?;
    let theirs = ready.get("version").and_then(Value::as_str);
    let result = match compare(theirs, env!("CARGO_PKG_VERSION"))? {
        Ordering::Less => {
            let mut result = push(stream, &mut reader, &ready)?;
            result["direction"] = json!("push");
            result
        }
        Ordering::Equal => finish_current(stream, &mut reader)?,
        Ordering::Greater => pull(stream, &mut reader, theirs.unwrap_or(""))?,
    };
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
    if arguments.len() == 1 && Api::Help.matches(&arguments[0]) {
        print!(
            "{}",
            cli_output::text(if upgrading { UPGRADE_HELP } else { HELP })
        );
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
    let mut result = match result {
        Ok(result) => result,
        Err(error) => json!({"ok":false,"error":error.to_string()}),
    };
    if upgrading && result.get("ok").and_then(Value::as_bool) != Some(false) {
        // Settings follow versions: once both sides run compatible code, merge their worminal.yaml.
        // A failed sync is reported beside the upgrade result but never fails the upgrade.
        if let [endpoint] = arguments {
            result["config"] = endpoint
                .to_str()
                .ok_or_else(|| invalid("SSH endpoint must be UTF-8."))
                .and_then(|endpoint| {
                    crate::config_sync::run(endpoint, worminal_config::sync::Prefer::Neither)
                })
                .unwrap_or_else(|error| json!({"ok": false, "error": error.to_string()}));
        }
    }
    if upgrading
        && (result.get("current").is_none()
            || result["config"]["conflicts"]
                .as_array()
                .is_some_and(|c| !c.is_empty()))
    {
        // Detached views discard stdout, so record changes, failures and conflicts where daemon events go.
        crate::process::diagnostic(&format!("Remote upgrade: {result}"));
    }
    print!("{}", worminal_workspace::cli_output::reply(&result));
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
    fn desktop_layout_names_the_modules_from_their_headers_in_publication_order() {
        let roles: Vec<_> = desktop_layout()
            .iter()
            .map(|file| file["role"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            roles,
            ["browser", "frontend", "launcher", "desktop", "icon"]
        );
        let paths: Vec<_> = desktop_layout()
            .iter()
            .filter_map(|file| file["path"].as_str().map(str::to_string))
            .collect();
        assert!(paths[0].starts_with("lib/worminal/") && paths[0].ends_with(".so"));
        assert_eq!(
            defined("#define A \"x.so\"\n", "A").as_deref(),
            Some("x.so")
        );
        assert_eq!(defined("#define AB \"x.so\"\n", "A"), None);
    }

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
