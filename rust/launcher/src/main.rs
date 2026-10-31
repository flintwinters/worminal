//! Lightweight native application entrypoint; graphics load only for a new local owner.
mod native;
use std::{
    ffi::OsString,
    io::{self, Read, Write},
    os::{
        fd::AsFd,
        linux::net::SocketAddrExt,
        unix::{
            ffi::OsStrExt,
            net::{SocketAddr, UnixListener, UnixStream},
            process::CommandExt,
        },
    },
    process::{Command, ExitCode},
    time::Duration,
};
use worminal_protocol::{Buffer, Packet, WIRE_HELLO, WORKSPACE_DAEMON};

enum Forward {
    Owner(UnixListener),
    Complete,
}

fn forward(args: &[OsString]) -> io::Result<Forward> {
    let mut key = Vec::new();
    for (index, name) in [
        "GDK_BACKEND",
        "WAYLAND_DISPLAY",
        "DISPLAY",
        "WORMINAL_SHARED_SOCKET_SCOPE",
    ]
    .iter()
    .enumerate()
    {
        if index != 0 {
            key.push(b':');
        }
        if let Some(value) = std::env::var_os(name) {
            key.extend_from_slice(value.as_bytes());
        }
    }
    let hash = native::hash(&key)?;
    let name = format!(
        "worminal-preview-ui-{}-{}",
        native::uid(),
        std::str::from_utf8(&hash[..48]).unwrap()
    );
    let address = SocketAddr::from_abstract_name(name.as_bytes())?;
    if let Ok(listener) = UnixListener::bind_addr(&address) {
        return Ok(Forward::Owner(listener));
    }
    let mut peer = UnixStream::connect_addr(&address)?;
    peer.set_read_timeout(Some(Duration::from_secs(5)))?;
    peer.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut payload = Buffer::default();
    payload.put_string(&native::current_directory()?)?;
    payload.put_u32(args.len() as u32)?;
    for arg in args {
        payload.put_string(arg.as_bytes())?;
    }
    let environment: Vec<_> = std::env::vars_os().collect();
    payload.put_u32(environment.len() as u32)?;
    for (key, value) in environment {
        let mut entry = key.into_encoded_bytes();
        entry.push(b'=');
        entry.extend_from_slice(value.as_bytes());
        payload.put_string(&entry)?;
    }
    worminal_workspace::rights::send_descriptor(&peer, io::stdout().as_fd(), 0)?;
    peer.write_all(
        &Packet {
            kind: WIRE_HELLO,
            tab: 0,
            body: payload.into_bytes(),
        }
        .encode()?,
    )?;
    let mut result = [0];
    peer.read_exact(&mut result)?;
    if result[0] != 0 {
        return Err(io::Error::other(
            "Could not open a window in the local GTK frontend.",
        ));
    }
    Ok(Forward::Complete)
}

fn run() -> io::Result<()> {
    native::timing("main");
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|arg| arg == "--new-browser-tab") {
        if args.len() != 3 {
            eprintln!("usage: worminal --new-browser-tab URL|FILE");
            std::process::exit(2);
        }
        let executable = std::env::current_exe()?;
        let daemon = std::ffi::CStr::from_bytes_with_nul(WORKSPACE_DAEMON).unwrap();
        let daemon = executable.with_file_name(std::ffi::OsStr::from_bytes(daemon.to_bytes()));
        return Err(Command::new(daemon)
            .arg("--new-browser-tab")
            .arg(&args[2])
            .exec());
    }
    let local = args.len() == 2
        && ["-h", "--help", "-v", "--version"]
            .iter()
            .any(|flag| args[1] == *flag);
    let listener = if local {
        None
    } else {
        let outcome = forward(&args);
        native::timing("broker");
        match outcome? {
            Forward::Complete => return Ok(()),
            Forward::Owner(listener) => Some(listener),
        }
    };
    native::enter(args, listener)
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
