//! Lightweight native application entrypoint; graphics load only for a new local owner.
#[path = "native.rs"]
mod native;
use std::{
    ffi::OsString,
    io,
    os::{linux::net::SocketAddrExt, unix::net::SocketAddr},
    process::ExitCode,
};
use worminal_workspace::broker::Forward;

fn forward(args: &[OsString], candidate: bool) -> io::Result<Forward> {
    let key = worminal_workspace::broker::display_key(candidate);
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
    let detach = worminal_workspace::broker::detach_master(&args)?.is_some();
    let local = args.len() == 2
        && ["-h", "--help", "-v", "--version"]
            .iter()
            .any(|flag| args[1] == *flag);
    let replacing = args.len() == 3 && args[1] == "--replace-owner";
    let listener = if local || replacing || worminal_workspace::ssh::is_askpass(&args) {
        None
    } else {
        let outcome = forward(&args, candidate);
        native::timing("broker");
        match outcome? {
            Forward::Complete => return Ok(()),
            Forward::Owner(_) if detach => {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "No connected GUI owner",
                ));
            }
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
