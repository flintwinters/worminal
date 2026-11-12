//! Runtime appearance and scrollback settings.
//!
//! Configuration is read when a process first needs it, never at build time. `worminal.yaml` holds the
//! Worminal-owned settings and may use any Alacritty key; an existing Alacritty config (TOML or legacy
//! YAML, with imports) supplies everything it does not override, so current setups keep working.
mod resolve;
mod source;
pub mod sync;

pub use resolve::{Settings, COLOR_COUNT};
pub use source::{alacritty_path, worminal_path};
use std::sync::OnceLock;

/// Settings of this process. Invalid configuration reports to stderr and falls back to defaults rather
/// than leaving a terminal that cannot start; `worminald config --check` is the strict validator.
pub fn get() -> &'static Settings {
    static SETTINGS: OnceLock<Settings> = OnceLock::new();
    SETTINGS.get_or_init(|| {
        load().unwrap_or_else(|error| {
            eprintln!("worminal: ignoring invalid configuration: {error}");
            Settings::resolve(&serde_json::Value::Null).expect("built-in defaults are valid")
        })
    })
}

/// Resolve the effective configuration from the discovered files.
pub fn load() -> Result<Settings, String> {
    Settings::resolve(&source::effective(
        alacritty_path()?.as_deref(),
        worminal_path()?.as_deref(),
    )?)
}

#[cfg(test)]
mod tests;
