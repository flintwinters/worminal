//! Extended agent CLI syntax. Payload encoding and target-dependent keys belong to Worp/the daemon.
use super::invalid;
use std::{
    ffi::OsString,
    io::{self, Read},
};
use worminal_protocol::worp::{Request, MAX_INPUT_BYTES};
fn tab(value: &str) -> io::Result<u32> {
    value
        .parse()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| invalid("TAB must be a positive ID."))
}
pub(super) fn parse(arguments: &[OsString]) -> io::Result<Option<Request>> {
    let args: Vec<_> = arguments
        .iter()
        .map(|a| {
            a.to_str()
                .ok_or_else(|| invalid("Arguments must be UTF-8."))
        })
        .collect::<io::Result<_>>()?;
    let Some(&operation) = args.first() else {
        return Ok(None);
    };
    if matches!(operation, "read" | "wait") {
        return observe(&args);
    }
    if !matches!(operation, "key" | "type") || args.len() < 3 {
        return Ok(None);
    }
    let id = tab(args[1])?;
    let mut rest = args[2..].to_vec();
    let mut guard = String::new();
    // Options precede payload; `--` disambiguates literal text that starts with an option.
    if rest.first() == Some(&"--guard") {
        guard = rest
            .get(1)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| invalid("--guard requires a nonempty input_guard token."))?
            .to_string();
        rest.drain(..2);
    }
    let literal = rest.first() == Some(&"--");
    if literal {
        rest.remove(0);
    }
    if operation == "key" {
        if rest.is_empty() {
            return Err(invalid("key requires at least one KEY."));
        }
        for name in &rest {
            worminal_engine::keys::named(name, 0).map_err(invalid)?;
        }
        return Ok(Some(Request::Keys {
            tab: id,
            names: rest.iter().map(|s| (*s).to_owned()).collect(),
            guard,
        }));
    }
    if !literal && rest == ["--stdin"] {
        let mut bytes = Vec::new();
        io::stdin()
            .lock()
            .take(MAX_INPUT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_INPUT_BYTES {
            return Err(invalid("stdin exceeds 65536 bytes."));
        }
        let text = String::from_utf8(bytes).map_err(|_| invalid("stdin must be UTF-8."))?;
        return Ok(Some(Request::Paste {
            tab: id,
            text,
            guard,
        }));
    }
    if rest.len() != 1 {
        return Err(invalid("Usage: type TAB [--guard TOKEN] TEXT|--stdin."));
    }
    if rest[0].chars().any(char::is_control) {
        return Err(invalid(
            "TEXT must contain no control characters; use key or --stdin.",
        ));
    }
    if guard.is_empty() {
        return Ok(Some(Request::Input {
            tab: id,
            bytes: rest[0].as_bytes().to_vec(),
        }));
    }
    Ok(Some(Request::GuardedInput {
        tab: id,
        bytes: rest[0].as_bytes().to_vec(),
        guard,
    }))
}

fn observe(args: &[&str]) -> io::Result<Option<Request>> {
    if args[0] == "read" {
        if args.len() == 2 {
            return Ok(Some(Request::Read { tab: tab(args[1])? }));
        }
        let mut tabs = Vec::new();
        let mut tail = 0;
        let mut i = 1;
        while i < args.len() {
            if args[i] == "--tail" && tail == 0 {
                i += 1;
                tail = args
                    .get(i)
                    .and_then(|s| s.parse::<u32>().ok())
                    .filter(|n| *n > 0)
                    .ok_or_else(|| invalid("--tail requires 1..1000 physical rows."))?;
            } else {
                tabs.push(tab(args[i])?);
            }
            i += 1;
        }
        return Ok(Some(Request::ReadMany { tabs, tail }));
    }
    if args[0] == "wait" {
        if args.len() != 6 || args[2] != "--after" || args[4] != "--timeout" {
            return Err(invalid(
                "Usage: wait TAB --after REVISION --timeout SECONDS (0..60).",
            ));
        }
        let seconds = args[5]
            .parse::<u32>()
            .ok()
            .filter(|n| *n <= 60)
            .ok_or_else(|| invalid("Timeout is 0..60 seconds."))?;
        return Ok(Some(Request::Wait {
            tab: tab(args[1])?,
            after: args[3].into(),
            timeout_ms: seconds * 1000,
        }));
    }
    unreachable!("observation command checked by caller")
}
