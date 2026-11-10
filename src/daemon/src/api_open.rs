use std::{collections::HashSet, ffi::OsString, io};
use worminal_protocol::worp::{tabs::TerminalOptions, Request};

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
fn text(value: &OsString) -> io::Result<String> {
    value
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| invalid("Open options/argv must be UTF-8."))
}

pub(super) fn parse(arguments: &[OsString]) -> io::Result<Request> {
    if arguments.first().is_some_and(|arg| arg == "--browser") {
        if arguments.len() != 2 {
            return Err(invalid("Use open --browser URL with no terminal options."));
        }
        let request = Request::OpenBrowser {
            url: text(&arguments[1])?,
        };
        request.validate()?;
        return Ok(request);
    }
    let mut options = TerminalOptions::default();
    let mut seen = HashSet::new();
    let mut index = 0;
    while index < arguments.len() {
        let flag = text(&arguments[index])?;
        if flag == "--" {
            if index + 1 == arguments.len() {
                return Err(invalid("Supply a program after --."));
            }
            options.command = arguments[index + 1..]
                .iter()
                .map(text)
                .collect::<io::Result<Vec<_>>>()?;
            break;
        }
        if !matches!(flag.as_str(), "--cwd" | "--title" | "--columns" | "--rows")
            || !seen.insert(flag.clone())
            || index + 1 == arguments.len()
        {
            return Err(invalid(
                "Invalid/duplicate open option; pass program argv after --. Run api help.",
            ));
        }
        let value = text(&arguments[index + 1])?;
        match flag.as_str() {
            "--cwd" => options.directory = value,
            "--title" => options.title = value,
            "--columns" => {
                options.columns = value
                    .parse()
                    .map_err(|_| invalid("Invalid column count."))?
            }
            "--rows" => options.rows = value.parse().map_err(|_| invalid("Invalid row count."))?,
            _ => unreachable!(),
        }
        index += 2;
    }
    let request = Request::OpenTerminal { options };
    request.validate()?;
    Ok(request)
}
