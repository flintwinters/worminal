//! Caller-specific launch configuration; forwarding never changes the GTK owner's environment.
use std::{
    ffi::OsString,
    io,
    os::{fd::AsFd, unix::ffi::OsStrExt},
    path::PathBuf,
    rc::Rc,
};
use worminal_protocol::*;
#[derive(Clone)]
pub(super) struct Config {
    pub(super) candidate: bool,
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
    pub(super) stream: Rc<std::fs::File>,
    pub(super) class: Option<String>,
    pub(super) name: Option<String>,
    pub(super) zoom: f64,
}
fn argument(message: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
impl Config {
    #[expect(clippy::too_many_lines, reason = "one arm per command-line option")]
    pub(super) fn parse(
        request: &worminal_workspace::broker::Request,
        candidate: bool,
    ) -> io::Result<Option<Self>> {
        let product = if candidate {
            "worminal-rust"
        } else {
            "worminal"
        };
        let version = env!("CARGO_PKG_VERSION");
        let mut config = Self {
            candidate,
            columns: 80,
            rows: 24,
            title: if candidate {
                "Worminal Rust"
            } else {
                "Worminal"
            }
            .into(),
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
            stream: Rc::new(std::fs::File::from(
                io::stdout().as_fd().try_clone_to_owned()?,
            )),
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
        let mut args = request.args.iter().skip(1).cloned();
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("--help" | "-h") => {
                    println!("{product} {version}\n\n\
                        usage: {product} [--master HOST] [--attach] [-aiv] [-f font] [-g COLSxROWS] \
                        [-c class] [-n name] [-T title] [-o file] [-l line] [-e command ...]\n\n\
                        Ctrl+Shift+T: tab  Ctrl+Shift+N: window  \
                        Ctrl+Shift+W: close tab\nCtrl+Shift+C/V: clipboard  Alt+1..9: select tab\n\n\
                        --surf URL|FILE: open a browser window (--browser is an alias)\n\
                        --new-browser-tab URL|FILE: replace this terminal with a browser\n\
                        Automation and agents: run `worminald --help`, then `worminald api help`\n\
                        -h | --help: show help\n-v | --version: show version");
                    return Ok(None);
                }
                Some("--version" | "-v") => {
                    println!("{product} {version}");
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
                        .ok_or_else(|| argument("option requires a value"))?;
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
                                .ok_or_else(|| argument("use COLSxROWS"))?;
                            config.columns =
                                cols.parse().map_err(|_| argument("invalid columns"))?;
                            config.rows = rows.parse().map_err(|_| argument("invalid rows"))?;
                            if config.columns == 0
                                || config.columns > WORKSPACE_MAX_COLS
                                || config.rows == 0
                                || config.rows > WORKSPACE_MAX_ROWS
                            {
                                return Err(argument("invalid geometry"));
                            }
                        }
                    }
                }
                Some(value) if value.starts_with('-') => {
                    return Err(argument(format!("unsupported option {value}; see --help")))
                }
                _ => {
                    config.command.push(arg);
                    config.command.extend(args);
                    break;
                }
            }
        }
        // Only -o - transfers the caller's stream. Ordinary forwarded launches use the owner's
        // stdout, including future Ctrl+Print, so their caller's output reader can observe EOF.
        // Keeping a default terminal's transferred pipe open deadlocks communicate()/shell pipelines.
        if config.output == "-" {
            config.stream = Rc::new(request.output.try_clone()?);
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

impl Config {
    pub(super) fn replacement_request(&self) -> io::Result<worminal_workspace::broker::Request> {
        use std::os::unix::ffi::OsStringExt;
        let mut args: Vec<OsString> = vec![
            "worminal".into(),
            "--attach".into(),
            "-T".into(),
            self.title.clone().into(),
            "-g".into(),
            format!("{}x{}", self.columns, self.rows).into(),
        ];
        if let Some(master) = &self.master {
            args.extend(["--master".into(), master.clone()]);
        }
        for (flag, value) in [("-f", &self.font), ("-c", &self.class), ("-n", &self.name)] {
            if let Some(value) = value {
                args.extend([flag.into(), value.into()]);
            }
        }
        if !self.alternate {
            args.push("-a".into());
        }
        if self.fixed {
            args.push("-i".into());
        }
        if self.output == "-" {
            args.extend(["-o".into(), "-".into()]);
        }
        let environment = self
            .environment
            .iter()
            .map(|(key, value)| {
                let mut bytes = key.as_os_str().as_bytes().to_vec();
                bytes.push(b'=');
                bytes.extend_from_slice(value.as_os_str().as_bytes());
                OsString::from_vec(bytes)
            })
            .collect();
        Ok(worminal_workspace::broker::Request {
            args,
            directory: if self.cwd.is_absolute() {
                self.cwd.clone()
            } else {
                "/".into()
            },
            environment,
            output: self.stream.try_clone()?,
        })
    }
}
