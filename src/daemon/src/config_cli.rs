//! `worminald config`: inspect and validate the runtime configuration without starting a service.
use std::{ffi::OsString, io};
use worminal_config::sync::Prefer;
use worminal_workspace::{
    cli_output,
    cli_schema::{Config, ConfigOption as Flag},
};

const USAGE: &str = "usage: worminald config [--json [--defaults]|--paths|--example|--help]\n\
    worminald config sync [--prefer local|remote] SSH_ENDPOINT\n\
    Run worminald config --help for the complete reference.";

pub(crate) fn reference() -> String {
    format!(
        "{}\n{}\n{}",
        Config::GROUP.reference(),
        Flag::GROUP.reference(),
        worminal_config::help()
    )
}

fn sync(arguments: &[OsString]) -> io::Result<i32> {
    let (prefer, endpoint) = match arguments {
        [endpoint] => (Prefer::Neither, endpoint),
        [flag, side, endpoint] if Flag::Prefer.matches(flag) => match side.to_str() {
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
    print!("{}", cli_output::reply(&result));
    Ok(i32::from(result["ok"] == false))
}

fn usage() -> io::Result<i32> {
    eprintln!("{}", cli_output::error(USAGE));
    Ok(2)
}

pub fn run(arguments: &[OsString]) -> io::Result<i32> {
    match arguments
        .first()
        .and_then(|word| word.to_str())
        .and_then(Config::parse)
    {
        Some(Config::Sync) => return sync(&arguments[1..]),
        Some(Config::Exchange) if arguments.len() == 1 => {
            crate::config_sync::exchange()?;
            return Ok(0);
        }
        _ => {}
    }
    let mode = match arguments {
        [] => None,
        [flag] if Flag::Json.matches(flag) || Flag::Paths.matches(flag) => {
            flag.to_str().and_then(Flag::parse)
        }
        [json, defaults] if Flag::Json.matches(json) && Flag::Defaults.matches(defaults) => {
            Some(Flag::Defaults)
        }
        [flag] if Flag::Help.matches(flag) => {
            println!("{}", cli_output::text(reference()));
            return Ok(0);
        }
        [flag] if Flag::Example.matches(flag) => {
            print!("{}", worminal_config::EXAMPLE);
            return Ok(0);
        }
        _ => {
            eprintln!("{}", cli_output::error(USAGE));
            return Ok(2);
        }
    };
    let found = |path: Result<Option<std::path::PathBuf>, String>| {
        path.map(|path| path.map_or_else(|| "none".to_string(), |path| path.display().to_string()))
    };
    let result = (|| {
        if mode == Some(Flag::Paths) {
            println!(
                "{}",
                cli_output::text(format!(
                    "worminal: {}",
                    found(worminal_config::worminal_path())?
                ))
            );
            println!(
                "{}",
                cli_output::text(format!(
                    "alacritty: {}",
                    found(worminal_config::alacritty_path())?
                ))
            );
            println!(
                "{}",
                cli_output::text(format!(
                    "worminal_default: {}",
                    worminal_config::worminal_default_path().display()
                ))
            );
            return Ok(());
        }
        let settings = if mode == Some(Flag::Defaults) {
            worminal_config::Settings::resolve(&serde_json::Value::Null)?
        } else {
            worminal_config::load()?
        };
        if mode == Some(Flag::Json) || mode == Some(Flag::Defaults) {
            println!("{}", settings.to_json());
        } else {
            println!("{}", cli_output::success("configuration is valid"));
            if let Some(path) = worminal_config::worminal_path()? {
                let ignored = worminal_config::unknown_keys(&worminal_config::read(&path)?);
                if !ignored.is_empty() {
                    println!(
                        "{}",
                        cli_output::text(format!(
                            "ignored by Worminal in {}: {} (see worminald config --help)",
                            path.display(),
                            ignored.join(", ")
                        ))
                    );
                }
            }
        }
        Ok::<(), String>(())
    })();
    match result {
        Ok(()) => Ok(0),
        Err(error) => {
            eprintln!(
                "{}",
                cli_output::error(format!("worminal: invalid configuration: {error}"))
            );
            Ok(1)
        }
    }
}
