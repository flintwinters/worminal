//! Lightweight native application entrypoint; graphics load only for a new local owner.
mod native;
use std::{
    ffi::OsString,
    io,
    os::{
        linux::net::SocketAddrExt,
        unix::{ffi::OsStrExt, net::SocketAddr},
    },
    process::ExitCode,
};
use worminal_workspace::broker::Forward;

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
    let request = worminal_workspace::broker::Request::capture(args.to_vec())?;
    worminal_workspace::broker::forward(&address, &request)
}

fn run() -> io::Result<()> {
    native::timing("main");
    let args: Vec<_> = std::env::args_os().collect();
    worminal_workspace::browser_target::command(&args)?;
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
            ExitCode::from(if error.kind() == io::ErrorKind::InvalidInput {
                2
            } else {
                1
            })
        }
    }
}
