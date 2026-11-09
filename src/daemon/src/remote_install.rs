//! Bootstrap a new headless node without copying credentials, project files or user state.
use serde_json::{json, Value};
use std::{
    ffi::OsString,
    fs::File,
    io::{self, BufReader, Read, Write},
    time::Duration,
};
use worminal_protocol::worp::devices::validate_endpoint;
use worminal_workspace::ssh;

const LAYOUT: &str = include_str!("../../../assets/headless-install.json");
const BOOTSTRAP: &str = include_str!("../../../tools/remote_install.py");
const MANUAL: &str = include_str!("../../../docs/man/worminal.1");
const LIMIT: u64 = 64 * 1024 * 1024;
const HEADER_LIMIT: usize = 16384;
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

fn install(endpoint: &str) -> io::Result<Value> {
    validate_endpoint(endpoint)?;
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
    let code = format!(
        "import json\nLAYOUT=json.loads({})\nSOURCE={}\nexec(SOURCE)\n",
        serde_json::to_string(LAYOUT).unwrap(),
        serde_json::to_string(BOOTSTRAP).unwrap()
    );
    let (mut stream, _child) = ssh::connect_python(endpoint.as_ref(), &code)?;
    stream.set_read_timeout(Some(Duration::from_secs(120)))?;
    stream.set_write_timeout(Some(Duration::from_secs(120)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    check_platform(&receive(&mut reader)?)?;
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
    if receive(&mut reader)?.get("accept").and_then(Value::as_bool) != Some(true) {
        return Err(invalid("Target did not accept the installation payload."));
    }
    for file in files {
        match file["role"].as_str() {
            Some("daemon") => {
                io::copy(&mut binary.try_clone()?.take(size), &mut stream)?;
            }
            Some("manual") => stream.write_all(&manual)?,
            _ => return Err(invalid("Unknown compiled installation resource.")),
        }
    }
    stream.flush()?;
    let mut result = receive(&mut reader)?;
    if result.get("installed").and_then(Value::as_bool) != Some(true) {
        return Err(invalid(
            "Target did not confirm installation; no automatic retry was made.",
        ));
    }
    result["endpoint"] = json!(endpoint);
    result["next"] = json!(format!("Open a view with: worminal --master {endpoint}"));
    Ok(result)
}

pub fn run(arguments: &[OsString]) -> io::Result<i32> {
    if arguments.len() == 1 && matches!(arguments[0].to_str(), Some("--help" | "-h" | "help")) {
        print!("{HELP}");
        return Ok(0);
    }
    let result = match arguments {
        [endpoint] => endpoint
            .to_str()
            .ok_or_else(|| invalid("SSH endpoint must be UTF-8."))
            .and_then(install),
        _ => Err(invalid("Run worminald install-on-remote --help for usage.")),
    };
    match result {
        Ok(result) => {
            println!("{result}");
            Ok(0)
        }
        Err(error) => {
            println!("{}", json!({"ok":false,"error":error.to_string()}));
            Ok(1)
        }
    }
}
