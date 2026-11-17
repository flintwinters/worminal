use crate::api::help;
use crate::api_help;
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
use worminal_workspace::{
    cli_output,
    cli_schema::{Api, ApiOption as Flag, Topic},
    socket, ssh,
};

#[path = "api_arguments.rs"]
mod arguments;
#[path = "api_diagnostics.rs"]
pub(crate) mod diagnostics;
#[path = "api_self.rs"]
mod identity;
#[path = "api_open.rs"]
mod open;

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.to_owned())
}

fn command(arguments: &[OsString]) -> io::Result<Request> {
    if let Some(request) = arguments::parse(arguments)? {
        request.validate()?;
        return Ok(request);
    }
    let operation = arguments
        .first()
        .and_then(|arg| arg.to_str())
        .and_then(Api::parse);
    let text = |index: usize| {
        arguments[index]
            .to_str()
            .ok_or_else(|| invalid("Arguments must be UTF-8."))
    };
    let request = match (operation, arguments.len()) {
        (Some(Api::List), 1) => Request::List,
        (Some(Api::Open), _) => open::parse(&arguments[1..])?,
        (Some(Api::Close), 2) => Request::Close {
            tab: text(1)?
                .parse()
                .map_err(|_| invalid("TAB must be a positive ID from api list."))?,
        },
        (Some(Api::Devices), 1) => Request::Devices,
        (Some(Api::Devices), 2) if Flag::NoCheck.matches(&arguments[1]) => Request::Devices,
        (Some(Api::Enroll), 3) => Request::Enroll {
            device: Device {
                name: text(1)?.into(),
                endpoint: text(2)?.into(),
            },
        },
        (Some(Api::Forget), 2) => Request::Forget {
            name: text(1)?.into(),
        },
        _ => {
            return Err(invalid(
                "Unknown command or arguments; run worminald api help.",
            ))
        }
    };
    request.validate()?;
    Ok(request)
}

fn request(scope: &[u8], master: Option<&OsStr>, request: Request) -> io::Result<Reply> {
    let seconds = match &request {
        Request::Wait { timeout_ms, .. } | Request::WaitCommand { timeout_ms, .. } => {
            u64::from(*timeout_ms).div_ceil(1000)
        }
        _ => 0,
    } + if master.is_some() { 13 } else { 3 };
    request_with_timeout(scope, master, request, seconds)
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
        Ok(
            request(self.0, None, operation)
                .unwrap_or_else(|error| diagnostics::io_failure(&error)),
        )
    }
}

pub fn run(scope: &[u8], arguments: &[OsString]) -> io::Result<i32> {
    if arguments.is_empty() || (arguments.len() == 1 && Api::Help.matches(&arguments[0])) {
        print!(
            "{}",
            cli_output::text(format!("{}{}", help(), api_help::index()))
        );
        return Ok(0);
    }
    if arguments.len() >= 2
        && Api::Help.matches(&arguments[0])
        && Topic::Search.matches(&arguments[1])
    {
        let query = arguments[2..]
            .iter()
            .map(|value| value.to_str())
            .collect::<Option<Vec<_>>>();
        let Some(query) = query else {
            eprintln!("{}", cli_output::error("Search query must be UTF-8."));
            return Ok(2);
        };
        let Some(text) = api_help::search(&query.join(" ")) else {
            eprintln!(
                "{}",
                cli_output::error(
                    "Usage: worminald api help search QUERY... (1..256 bytes, at most 16 terms)"
                )
            );
            return Ok(2);
        };
        print!("{}", cli_output::text(text));
        return Ok(0);
    }
    if arguments.len() == 2 && Api::Help.matches(&arguments[0]) {
        let Some(topic) = arguments[1].to_str().and_then(api_help::topic) else {
            eprintln!(
                "{}",
                cli_output::error(format!("Unknown help topic.\n{}", api_help::index()))
            );
            return Ok(2);
        };
        if topic.runnable {
            print!("{}", topic.text);
        } else {
            print!("{}", cli_output::text(topic.text));
        }
        return Ok(0);
    }
    if arguments.len() == 1 && Api::Session.matches(&arguments[0]) {
        crate::cli::bridge(scope, true)?;
        return Ok(0);
    }
    if arguments.len() == 1 && Api::Stdio.matches(&arguments[0]) {
        stream::serve_once(io::stdin().lock(), io::stdout().lock(), &mut Local(scope))?;
        return Ok(0);
    }
    let (master, arguments) = if arguments
        .first()
        .is_some_and(|arg| Flag::Master.matches(arg))
    {
        (
            arguments.get(1).map(OsString::as_os_str),
            arguments.get(2..).unwrap_or_default(),
        )
    } else {
        (None, arguments)
    };
    if arguments.len() == 1 && Api::SelfTab.matches(&arguments[0]) {
        return Ok(print_result(identify_self(scope, master)));
    }
    if arguments.len() == 2
        && Api::Doctor.matches(&arguments[0])
        && Flag::Json.matches(&arguments[1])
    {
        let reply = diagnostics::doctor(scope, master);
        return Ok(print_result(Ok((reply.json, reply.success))));
    }
    let result = resolve_self(scope, master, arguments)
        .and_then(|args| command(&args))
        .and_then(|operation| {
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
    Ok(print_result(
        result.map(|reply| (reply.json, reply.success)),
    ))
}

/// Resolve only tab operand positions; literal payloads named "self" stay literal.
fn resolve_self(
    scope: &[u8],
    master: Option<&OsStr>,
    arguments: &[OsString],
) -> io::Result<Vec<OsString>> {
    let mut args = arguments.to_vec();
    let operation = args.first().and_then(|a| a.to_str()).unwrap_or("");
    let positions: Vec<_> = match operation {
        "read" => {
            let mut positions = Vec::new();
            let mut i = 1;
            while i < args.len() {
                if args[i].to_string_lossy().starts_with("--") {
                    i += 2;
                } else {
                    positions.push(i);
                    i += 1;
                }
            }
            positions
        }
        "close" | "wait" | "type" | "key" => vec![1],
        _ => Vec::new(),
    };
    let mut id = None;
    for position in positions {
        if args.get(position).is_some_and(|a| a == "self") {
            if id.is_none() {
                let (json, success) = identify_self(scope, master)?;
                if !success {
                    return Err(io::Error::other(json));
                }
                let value: serde_json::Value = serde_json::from_str(&json)?;
                id = Some(
                    value["id"]
                        .as_u64()
                        .ok_or_else(|| invalid("Invalid self reply."))?
                        .to_string(),
                );
            }
            args[position] = id.as_ref().unwrap().into();
        }
    }
    Ok(args)
}

fn identify_self(scope: &[u8], master: Option<&OsStr>) -> io::Result<(String, bool)> {
    if master.is_some() {
        return Err(invalid(
            "self identifies this machine's process; omit --master.",
        ));
    }
    let list = request(scope, None, Request::List)?;
    if !list.success {
        return Ok((list.json, false));
    }
    let chain = identity::ancestors(std::process::id());
    Ok((identity::identify(&list.json, &chain)?, true))
}

/// Prints the one JSON object every command promises and returns the exit status.
fn print_result(result: io::Result<(String, bool)>) -> i32 {
    match result {
        Ok((json, success)) => {
            println!("{json}");
            i32::from(!success)
        }
        Err(error) => {
            println!("{}", diagnostics::io_failure(&error).json);
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::quote;

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
        assert_eq!(
            command(&args(&["type", "12", "--", "--stdin"])).unwrap(),
            Request::Input {
                tab: 12,
                bytes: b"--stdin".to_vec()
            }
        );
        let packet = command(&args(&["key", "12", "Enter", "Ctrl-C"])).unwrap();
        assert_eq!(
            packet,
            Request::Keys {
                tab: 12,
                names: vec!["Enter".into(), "Ctrl-C".into()],
                guard: String::new()
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
    fn open_and_close_are_explicit_and_preserve_argv() {
        let Request::OpenTerminal { options } = command(&args(&[
            "open",
            "--cwd",
            "/target",
            "--",
            "/bin/echo",
            "literal ; $HOME",
        ]))
        .unwrap() else {
            panic!("terminal expected");
        };
        assert_eq!(options.directory, "/target");
        assert_eq!(options.command, ["/bin/echo", "literal ; $HOME"]);
        assert!(matches!(
            command(&args(&["close", "9"])).unwrap(),
            Request::Close { tab: 9 }
        ));
        for values in [
            vec!["open", "--cwd", "relative"],
            vec!["open", "--"],
            vec!["open", "--rows", "0"],
            vec!["open", "--browser", "relative.html"],
            vec!["open", "--title", "a", "--title", "b"],
            vec!["close", "0"],
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
