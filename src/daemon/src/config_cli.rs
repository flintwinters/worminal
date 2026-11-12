//! `worminald config`: inspect and validate the runtime configuration without starting a service.
use std::{ffi::OsString, io};

const USAGE: &str = "usage: worminald config [--json [--defaults]|--paths]\n\
    Validate the effective configuration (worminal.yaml layered over an Alacritty config).\n\
    --json  print the resolved settings (--defaults ignores every file); --paths  print the files in use";

pub fn run(arguments: &[OsString]) -> io::Result<i32> {
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
