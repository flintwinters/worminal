use crate::api::{quote, HELP};
use std::{
    ffi::{OsStr, OsString},
    io,
    time::Duration,
};
use worminal_protocol::worp::{
    devices::Device,
    stream::{self, Stream},
    Client, Handler, Reply, Request,
};
use worminal_workspace::{socket, ssh};

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.to_owned())
}

fn key(name: &str) -> io::Result<Vec<u8>> {
    match name.to_ascii_lowercase().as_str() {
        "enter" => Ok(vec![b'\r']),
        "tab" => Ok(vec![b'\t']),
        "escape" | "esc" => Ok(vec![27]),
        "backspace" => Ok(vec![127]),
        name if name.starts_with("ctrl-")
            && name.len() == 6
            && name.as_bytes()[5].is_ascii_lowercase() =>
        {
            Ok(vec![name.as_bytes()[5] - b'a' + 1])
        }
        _ => Err(invalid(
            "Unknown key. Use Enter, Tab, Escape, Backspace, or Ctrl-A through Ctrl-Z.",
        )),
    }
}

fn command(arguments: &[OsString]) -> io::Result<Request> {
    let operation = arguments.first().and_then(|arg| arg.to_str()).unwrap_or("");
    let (is_read, tab, input) = match (operation, arguments.len()) {
        ("list", 1) => return Ok(Request::List),
        ("devices", 1) => return Ok(Request::Devices),
        ("devices", 2) if arguments[1] == "--no-check" => return Ok(Request::Devices),
        ("enroll", 3) => {
            let device = Device {
                name: arguments[1]
                    .to_str()
                    .ok_or_else(|| invalid("NAME must be UTF-8."))?
                    .to_owned(),
                endpoint: arguments[2]
                    .to_str()
                    .ok_or_else(|| invalid("ENDPOINT must be UTF-8."))?
                    .to_owned(),
            };
            device.validate()?;
            return Ok(Request::Enroll { device });
        }
        ("forget", 2) => {
            let request = Request::Forget {
                name: arguments[1]
                    .to_str()
                    .ok_or_else(|| invalid("NAME must be UTF-8."))?
                    .to_owned(),
            };
            request.validate()?;
            return Ok(request);
        }
        ("read", 2) | ("type", 3) | ("key", 3..) => {
            let tab = arguments[1]
                .to_str()
                .and_then(|id| id.parse::<u32>().ok())
                .filter(|id| *id != 0)
                .ok_or_else(|| invalid("TAB must be a positive ID from worminald api list."))?;
            let input = match operation {
                "type" => {
                    let text = arguments[2]
                        .to_str()
                        .ok_or_else(|| invalid("TEXT must be UTF-8."))?;
                    // Literal typing cannot silently smuggle Enter or other control keystrokes.
                    // Use the explicit key operation to submit a prompt or interrupt a process.
                    if text.chars().any(char::is_control) {
                        return Err(invalid(
                            "TEXT must contain no control characters; use worminald api key.",
                        ));
                    }
                    text.as_bytes().to_vec()
                }
                "key" => {
                    let mut input = Vec::new();
                    for value in &arguments[2..] {
                        input.extend(key(value
                            .to_str()
                            .ok_or_else(|| invalid("KEY must be UTF-8."))?)?);
                    }
                    input
                }
                _ => Vec::new(),
            };
            (operation == "read", tab, input)
        }
        _ => {
            return Err(invalid(
                "Unknown command or arguments; run worminald api help.",
            ))
        }
    };
    let request = if is_read {
        Request::Read { tab }
    } else {
        Request::Input { tab, bytes: input }
    };
    request.validate()?;
    Ok(request)
}

fn request(scope: &[u8], master: Option<&OsStr>, request: Request) -> io::Result<Reply> {
    request_with_timeout(
        scope,
        master,
        request,
        if master.is_some() { 13 } else { 3 },
    )
}

fn request_with_timeout(
    scope: &[u8],
    master: Option<&OsStr>,
    request: Request,
    seconds: u64,
) -> io::Result<Reply> {
    let (socket, _ssh) = if let Some(master) = master {
        let (socket, child) = ssh::connect_worp(master)?;
        (socket, Some(child))
    } else {
        (
            socket::connect(crate::process::uid(), scope).map_err(|error| {
                io::Error::new(
                    error.kind(),
                    "No workspace service is running in this scope.",
                )
            })?,
            None,
        )
    };
    let timeout = Some(Duration::from_secs(seconds));
    socket.set_read_timeout(timeout)?;
    socket.set_write_timeout(timeout)?;
    Client::new(Stream::new(socket.try_clone()?, socket)).request(request)
}

struct Local<'a>(&'a [u8]);
impl Handler for Local<'_> {
    fn handle(&mut self, operation: Request) -> io::Result<Reply> {
        Ok(request(self.0, None, operation)
            .unwrap_or_else(|error| crate::api::failure(&error.to_string())))
    }
}

pub fn run(scope: &[u8], arguments: &[OsString]) -> io::Result<i32> {
    if arguments.is_empty()
        || (arguments.len() == 1 && matches!(arguments[0].to_str(), Some("help" | "--help" | "-h")))
    {
        print!("{HELP}");
        return Ok(0);
    }
    if arguments.len() == 1 && arguments[0] == "--stdio" {
        stream::serve_once(io::stdin().lock(), io::stdout().lock(), &mut Local(scope))?;
        return Ok(0);
    }
    let (master, arguments) = if arguments.first().is_some_and(|arg| arg == "--master") {
        (
            arguments.get(1).map(OsString::as_os_str),
            arguments.get(2..).unwrap_or_default(),
        )
    } else {
        (None, arguments)
    };
    let result = command(arguments).and_then(|operation| {
        let devices = matches!(operation, Request::Devices);
        let mut reply = request(scope, master, operation)?;
        if devices && reply.success {
            let mut value: serde_json::Value = serde_json::from_str(&reply.json)
                .map_err(|_| invalid("Invalid registry JSON reply."))?;
            crate::device_probes::validate(&value)?;
            value["registry_authority"] =
                serde_json::json!(master.map_or("local".into(), |host| host.to_string_lossy()));
            if arguments.len() == 1 {
                crate::device_probes::check(&mut value, |endpoint| {
                    request_with_timeout(scope, Some(OsStr::new(endpoint)), Request::List, 3)
                })?;
            }
            reply.json = value.to_string();
        }
        Ok(reply)
    });
    match result {
        Ok(reply) => {
            println!("{}", reply.json);
            Ok(i32::from(!reply.success))
        }
        Err(error) => {
            println!(
                "{{\"ok\":false,\"error\":{}}}",
                quote(error.to_string().as_bytes())
            );
            Ok(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn exact_arguments_and_explicit_submission() {
        let packet = command(&args(&["type", "12", "hello 世界"])).unwrap();
        assert_eq!(
            packet,
            Request::Input {
                tab: 12,
                bytes: "hello 世界".as_bytes().to_vec()
            }
        );
        let packet = command(&args(&["key", "12", "Enter", "Ctrl-C"])).unwrap();
        assert_eq!(
            packet,
            Request::Input {
                tab: 12,
                bytes: b"\r\x03".to_vec()
            }
        );
        for values in [
            vec!["type", "0", "text"],
            vec!["type", "1", "oops\n"],
            vec!["key", "1", "Unknown"],
            vec!["read", "1", "extra"],
            vec!["list", "extra"],
        ] {
            assert!(command(&args(&values)).is_err());
        }
    }

    #[test]
    fn quote_escapes_json_controls_and_keeps_unicode() {
        assert_eq!(quote(b"a\n\"\\\0"), "\"a\\n\\\"\\\\\\u0000\"");
        assert_eq!(quote("世界".as_bytes()), "\"世界\"");
        assert_eq!(quote(&[255]), "\"�\"");
    }
}
