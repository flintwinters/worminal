//! Configuration file discovery, parsing and layering.
use serde_json::{Map, Value};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

const ALACRITTY_NAMES: [&str; 3] = ["alacritty.toml", "alacritty.yml", "alacritty.yaml"];

fn home() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

pub(crate) fn config_home() -> PathBuf {
    env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map_or_else(|| home().join(".config"), PathBuf::from)
}

pub(crate) fn state_home() -> PathBuf {
    env::var_os("XDG_STATE_HOME")
        .filter(|value| !value.is_empty())
        .map_or_else(|| home().join(".local/state"), PathBuf::from)
}

fn explicit(variable: &str, what: &str) -> Result<Option<PathBuf>, String> {
    let Some(value) = env::var_os(variable).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let path = expand(&value.to_string_lossy());
    if path.is_file() {
        Ok(Some(path))
    } else {
        Err(format!("{what} does not exist: {}", path.display()))
    }
}

/// Where worminal.yaml belongs when none exists yet.
pub fn worminal_default_path() -> PathBuf {
    config_home().join("worminal/worminal.yaml")
}

/// `WORMINAL_CONFIG`, else `$XDG_CONFIG_HOME/worminal/worminal.yaml` when present.
pub fn worminal_path() -> Result<Option<PathBuf>, String> {
    if let Some(path) = explicit("WORMINAL_CONFIG", "Worminal config")? {
        return Ok(Some(path));
    }
    let path = worminal_default_path();
    Ok(path.is_file().then_some(path))
}

/// Alacritty's own search order, so an existing setup is found without any Worminal configuration.
pub fn alacritty_path() -> Result<Option<PathBuf>, String> {
    if let Some(path) = explicit("WORMINAL_ALACRITTY_CONFIG", "Alacritty config")? {
        return Ok(Some(path));
    }
    let home = home();
    let directories = [
        config_home().join("alacritty"),
        config_home(),
        home.join(".config/alacritty"),
    ];
    let mut candidates = Vec::new();
    for directory in &directories {
        candidates.extend(ALACRITTY_NAMES.iter().map(|name| directory.join(name)));
    }
    candidates.extend(
        ALACRITTY_NAMES
            .iter()
            .map(|name| home.join(format!(".{name}"))),
    );
    candidates.extend(
        ALACRITTY_NAMES
            .iter()
            .map(|name| Path::new("/etc/alacritty").join(name)),
    );
    Ok(candidates.into_iter().find(|path| path.is_file()))
}

fn expand(text: &str) -> PathBuf {
    match text.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None => PathBuf::from(text),
    }
}

/// Recursive object merge where later layers win key by key; non-objects replace.
pub fn merge(target: &mut Value, source: &Value) {
    match (target, source) {
        (Value::Object(target), Value::Object(source)) => {
            for (key, value) in source {
                match target.get_mut(key) {
                    Some(existing) => merge(existing, value),
                    None => {
                        target.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        (target, source) => *target = source.clone(),
    }
}

/// Parse one file (TOML or YAML by extension) together with its imports; the file wins over them.
pub fn read(path: &Path) -> Result<Value, String> {
    read_nested(path, &mut Vec::new())
}

fn read_nested(path: &Path, stack: &mut Vec<PathBuf>) -> Result<Value, String> {
    let path = fs::canonicalize(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if stack.contains(&path) {
        return Err(format!("import cycle: {}", path.display()));
    }
    let text = fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let yaml = matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("yml" | "yaml")
    );
    let config: Value = if yaml {
        serde_yaml_ng::from_str(&text).map_err(|error| error.to_string())
    } else {
        toml::from_str(&text).map_err(|error| error.to_string())
    }
    .map_err(|error| format!("{}: {error}", path.display()))?;
    let config = if config.is_null() {
        Value::Object(Map::new())
    } else {
        config
    };
    if !config.is_object() {
        return Err(format!("config must be a mapping: {}", path.display()));
    }
    let imports = config
        .pointer("/general/import")
        .or_else(|| config.get("import"));
    let imports = match imports {
        None => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| item.as_str().map(expand))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| format!("imports must be a list of paths: {}", path.display()))?,
        Some(_) => {
            return Err(format!(
                "imports must be a list of paths: {}",
                path.display()
            ))
        }
    };
    stack.push(path.clone());
    let mut result = Value::Object(Map::new());
    for import in imports {
        let import = if import.is_absolute() {
            import
        } else {
            path.parent().unwrap_or(Path::new("/")).join(import)
        };
        if import.is_file() {
            merge(&mut result, &read_nested(&import, stack)?);
        }
    }
    stack.pop();
    merge(&mut result, &config);
    Ok(result)
}

/// Layers: Alacritty config, then `worminal.yaml` over it.
pub fn effective(alacritty: Option<&Path>, worminal: Option<&Path>) -> Result<Value, String> {
    let mut result = Value::Object(Map::new());
    for path in [alacritty, worminal].into_iter().flatten() {
        merge(&mut result, &read(path)?);
    }
    Ok(result)
}
