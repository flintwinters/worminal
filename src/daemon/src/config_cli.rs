//! `worminald config`: inspect and validate the runtime configuration without starting a service.
use std::{ffi::OsString, io};
use worminal_config::sync::Prefer;

const USAGE: &str = "usage: worminald config [--json [--defaults]|--paths]\n\
    worminald config sync [--prefer local|remote] SSH_ENDPOINT\n\
    Validate the effective configuration (worminal.yaml layered over an Alacritty config).\n\
    --json  print the resolved settings (--defaults ignores every file); --paths  print the files in use\n\
    sync merges worminal.yaml with the peer per key against the last agreed copy; conflicting keys stay as they\n\
    are on both sides and are reported until --prefer picks a side. Comments in rewritten files are not kept.\n\
    (exchange is the peer side of sync.)";

fn sync(arguments: &[OsString]) -> io::Result<i32> {
    let (prefer, endpoint) = match arguments {
        [endpoint] => (Prefer::Neither, endpoint),
        [flag, side, endpoint] if flag == "--prefer" => match side.to_str() {
            Some("local") => (Prefer::Local, endpoint),
            Some("remote") => (Prefer::Remote, endpoint),
            _ => return usage(),
        },
        _ => return usage(),
    };
    let result = endpoint
        .to_str()
        .ok_or_else(|| io::Error::other("SSH endpoint must be UTF-8."))
        .and_then(|endpoint| crate::config_sync::run(endpoint, prefer))
        .unwrap_or_else(|error| serde_json::json!({"ok": false, "error": error.to_string()}));
    println!("{result}");
    Ok(i32::from(result["ok"] == false))
}

fn usage() -> io::Result<i32> {
    eprintln!("{USAGE}");
    Ok(2)
}

pub fn run(arguments: &[OsString]) -> io::Result<i32> {
    match arguments.first().and_then(|word| word.to_str()) {
        Some("sync") => return sync(&arguments[1..]),
        Some("exchange") if arguments.len() == 1 => {
            crate::config_sync::exchange()?;
            return Ok(0);
        }
        _ => {}
    }
    let mode = match arguments {
        [] => "",
        [flag] if flag == "--json" || flag == "--paths" => flag.to_str().unwrap_or(""),
        [json, defaults] if json == "--json" && defaults == "--defaults" => "--defaults",
        [flag] if flag == "--help" || flag == "-h" => {
            println!("{USAGE}");
            return Ok(0);
        }
        _ => {
            eprintln!("{USAGE}");
            return Ok(2);
        }
    };
    let found = |path: Result<Option<std::path::PathBuf>, String>| {
        path.map(|path| path.map_or_else(|| "none".to_string(), |path| path.display().to_string()))
    };
    let result = (|| {
        if mode == "--paths" {
            println!("worminal: {}", found(worminal_config::worminal_path())?);
            println!("alacritty: {}", found(worminal_config::alacritty_path())?);
            return Ok(());
        }
        let settings = if mode == "--defaults" {
            worminal_config::Settings::resolve(&serde_json::Value::Null)?
        } else {
            worminal_config::load()?
        };
        if mode == "--json" || mode == "--defaults" {
            println!("{}", settings.to_json());
        } else {
            println!("configuration is valid");
        }
        Ok::<(), String>(())
    })();
    match result {
        Ok(()) => Ok(0),
        Err(error) => {
            eprintln!("worminal: invalid configuration: {error}");
            Ok(1)
        }
    }
}
