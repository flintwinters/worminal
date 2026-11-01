//! Caller-specific launch configuration; forwarding never changes the GTK owner's environment.
use std::{ffi::OsString, io, os::unix::ffi::OsStrExt, path::PathBuf, rc::Rc};
use worminal_protocol::*;
#[derive(Clone)]
pub(super) struct Config {
    pub(super) columns: u32,
    pub(super) rows: u32,
    pub(super) title: String,
    pub(super) font: Option<String>,
    pub(super) cwd: PathBuf,
    pub(super) command: Vec<OsString>,
    pub(super) attach: bool,
    pub(super) alternate: bool,
    pub(super) fixed: bool,
    pub(super) output: OsString,
    pub(super) line: OsString,
    pub(super) master: Option<OsString>,
    pub(super) browser_url: Option<String>,
    pub(super) environment: Vec<(OsString, OsString)>,
    pub(super) stream: Option<Rc<std::fs::File>>,
    pub(super) class: Option<String>,
    pub(super) name: Option<String>,
    pub(super) zoom: f64,
}
impl Config {
    pub(super) fn parse(request: &worminal_workspace::broker::Request) -> io::Result<Option<Self>> {
        let mut config = Self {
            columns: 80,
            rows: 24,
            title: "Worminal Rust".into(),
            font: None,
            cwd: request.directory.clone(),
            command: Vec::new(),
            attach: false,
            alternate: true,
            fixed: false,
            output: OsString::new(),
            line: OsString::new(),
            master: None,
            browser_url: None,
            stream: Some(Rc::new(request.output.try_clone()?)),
            class: None,
            name: None,
            zoom: 0.0,
            environment: request
                .environment
                .iter()
                .cloned()
                .filter_map(|entry| {
                    use std::os::unix::ffi::OsStringExt;
                    let bytes = entry.into_encoded_bytes();
                    let split = bytes.iter().position(|byte| *byte == b'=')?;
                    Some((
                        OsString::from_vec(bytes[..split].to_vec()),
                        OsString::from_vec(bytes[split + 1..].to_vec()),
                    ))
                })
                .collect(),
        };
        let mut args = request.args.iter().cloned().skip(1);
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("--help" | "-h") => {
                    println!("worminal-rust: Rust GTK terminal\n\
                        usage: worminal-rust [--master HOST] [--attach] [-aiv] [-f font] [-g COLSxROWS] \
                        [-c class] [-n name] [-T title] [-o file] [-l line] [-e command ...]\n\n\
                        Ctrl+Shift+T: tab  Ctrl+Shift+N: window  \
                        Ctrl+Shift+W: close tab\nCtrl+Shift+C/V: clipboard  Alt+1..9: select tab\n\n\
                        --browser URL|FILE: open a browser tab in this window");
                    return Ok(None);
                }
                Some("--version" | "-v") => {
                    println!("worminal-rust {}", env!("WORMINAL_VERSION"));
                    return Ok(None);
                }
                Some("--attach") => config.attach = true,
                Some("-a") => config.alternate = false,
                Some("-i") => config.fixed = true,
                Some("-e" | "--") => {
                    config.command.extend(args);
                    break;
                }
                Some(
                    "--browser" | "--surf" | "--master" | "-c" | "-n" | "-T" | "-t" | "-f" | "-g"
                    | "-o" | "-l",
                ) => {
                    let value = args
                        .next()
                        .ok_or_else(|| io::Error::other("option requires a value"))?;
                    match arg.to_str().unwrap() {
                        "-c" => config.class = Some(value.to_string_lossy().into_owned()),
                        "-n" => config.name = Some(value.to_string_lossy().into_owned()),
                        "--browser" | "--surf" => {
                            let url = worminal_workspace::browser_target::resolve_in(
                                Some(&value),
                                &request.directory,
                            )?;
                            config.browser_url =
                                Some(String::from_utf8(url).map_err(io::Error::other)?);
                        }
                        "--master" => {
                            config.master = Some(value);
                            config.cwd = PathBuf::new();
                        }
                        "-T" | "-t" => config.title = value.to_string_lossy().into_owned(),
                        "-f" => config.font = Some(value.to_string_lossy().into_owned()),
                        "-o" => config.output = value,
                        "-l" => config.line = value,
                        _ => {
                            let text = value.to_string_lossy();
                            let (cols, rows) = text
                                .split_once('x')
                                .ok_or_else(|| io::Error::other("use COLSxROWS"))?;
                            config.columns = cols
                                .parse()
                                .map_err(|_| io::Error::other("invalid columns"))?;
                            config.rows =
                                rows.parse().map_err(|_| io::Error::other("invalid rows"))?;
                            if config.columns == 0
                                || config.columns > WORKSPACE_MAX_COLS
                                || config.rows == 0
                                || config.rows > WORKSPACE_MAX_ROWS
                            {
                                return Err(io::Error::other("invalid geometry"));
                            }
                        }
                    }
                }
                Some(value) if value.starts_with('-') => {
                    return Err(io::Error::other(format!(
                        "unsupported option {value}; see --help"
                    )))
                }
                _ => {
                    config.command.push(arg);
                    config.command.extend(args);
                    break;
                }
            }
        }
        // Browser launches have no printer stream. Retaining the forwarded pipe would prevent
        // the short-lived caller's stdout reader from seeing EOF after a successful launch.
        if config.browser_url.is_some() {
            config.stream = None;
        }
        Ok(Some(config))
    }
    pub(super) fn launch(&self) -> io::Result<Vec<u8>> {
        // Remote shells inherit the remote service's environment, never local PATH/HOME or restore markers.
        let environment: Vec<_> = if self.master.is_none() {
            self.environment.clone()
        } else {
            Vec::new()
        };
        let mut output = Buffer::default();
        for value in [
            self.columns,
            self.rows,
            u32::from(self.alternate),
            self.command.len() as u32,
            environment.len() as u32,
        ] {
            output.put_u32(value)?;
        }
        for value in [
            self.title.as_bytes(),
            self.cwd.as_os_str().as_bytes(),
            self.line.as_bytes(),
            self.output.as_bytes(),
            b"",
        ] {
            output.put_string(value)?;
        }
        for arg in &self.command {
            output.put_string(arg.as_bytes())?;
        }
        for (key, value) in environment {
            let mut entry = key.into_encoded_bytes();
            entry.push(b'=');
            entry.extend_from_slice(value.as_bytes());
            output.put_string(&entry)?;
        }
        Ok(output.into_bytes())
    }
}
