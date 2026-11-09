//! Lightweight native application entrypoint; graphics load only for a new local owner.
#[path = "native.rs"]
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

fn forward(args: &[OsString], candidate: bool) -> io::Result<Forward> {
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
        let value = std::env::var_os(name).unwrap_or_default();
        if candidate {
            key.extend_from_slice(&(value.as_bytes().len() as u64).to_be_bytes());
        } else if index != 0 {
            key.push(b':');
        }
        key.extend_from_slice(value.as_bytes());
    }
    let hash = native::hash(&key)?;
    let name = format!(
        "{}-{}-{}",
        if candidate {
            "worminal-rust-ui"
        } else {
            "worminal-preview-ui"
        },
        native::uid(),
        std::str::from_utf8(&hash[..48]).unwrap()
    );
    let address = SocketAddr::from_abstract_name(name.as_bytes())?;
    let request = worminal_workspace::broker::Request::capture(args.to_vec())?;
    worminal_workspace::broker::forward(&address, &request)
}

fn run(candidate: bool) -> io::Result<()> {
    native::timing("main");
    let args: Vec<_> = std::env::args_os().collect();
    worminal_workspace::browser_target::command(&args)?;
    let local = args.len() == 2
        && ["-h", "--help", "-v", "--version"]
            .iter()
            .any(|flag| args[1] == *flag);
    let replacing = args.len() == 3 && args[1] == "--replace-owner";
    let listener = if local || replacing {
        None
    } else {
        let outcome = forward(&args, candidate);
        native::timing("broker");
        match outcome? {
            Forward::Complete => return Ok(()),
            Forward::Owner(listener) => Some(listener),
        }
    };
    native::enter(args, listener, candidate)
}

pub fn main(candidate: bool) -> ExitCode {
    match run(candidate) {
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
