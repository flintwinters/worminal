use serde_json::{Map, Value as JsonValue};
use std::{collections::HashSet, path::{Path, PathBuf}};
use toml::Value;

const BASIC_COLORS: [&str; 8] = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
];

fn config_path() -> Option<PathBuf> {
    if cfg!(windows) {
        return std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .map(|path| path.join("alacritty/alacritty.toml"))
            .filter(|path| path.is_file());
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
    let mut paths = Vec::new();
    if let Some(xdg) = xdg {
        paths.push(xdg.join("alacritty/alacritty.toml"));
        paths.push(xdg.join("alacritty.toml"));
    }
    if let Some(home) = home {
        paths.push(home.join(".config/alacritty/alacritty.toml"));
        paths.push(home.join(".alacritty.toml"));
    }
    paths.push(PathBuf::from("/etc/alacritty/alacritty.toml"));
    paths.into_iter().find(|path| path.is_file())
}

fn resolve_import(base: &Path, import: &str) -> PathBuf {
    if let Some(relative) = import.strip_prefix("~/") {
        std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(relative)
    } else if Path::new(import).is_absolute() {
        PathBuf::from(import)
    } else {
        base.parent().unwrap_or(Path::new(".")).join(import)
    }
}

// Alacritty loads imports in order, then the importing file. Merge each color
// table by key so a local foreground does not discard an imported background.
fn read_colors(path: &Path, visited: &mut HashSet<PathBuf>, result: &mut Value) {
    let Ok(path) = path.canonicalize() else { return };
    if !visited.insert(path.clone()) { return; }
    let Ok(content) = std::fs::read_to_string(&path) else { return };
    let Ok(config) = content.parse::<Value>() else { return };
    let imports = config.get("general").and_then(|general| general.get("import"))
        .or_else(|| config.get("import"))
        .and_then(Value::as_array);
    if let Some(imports) = imports {
        for import in imports.iter().filter_map(Value::as_str) {
            read_colors(&resolve_import(&path, import), visited, result);
        }
    }
    if let Some(colors) = config.get("colors") {
        merge(result, colors);
    }
}

fn merge(target: &mut Value, source: &Value) {
    if let (Some(target), Some(source)) = (target.as_table_mut(), source.as_table()) {
        for (key, value) in source {
            if let Some(existing) = target.get_mut(key) {
                merge(existing, value);
            } else {
                target.insert(key.clone(), value.clone());
            }
        }
    } else {
        *target = source.clone();
    }
}

fn color(colors: &Value, group: &str, key: &str) -> Option<String> {
    let value = colors.get(group)?.get(key)?.as_str()?;
    if value.len() == 7 && value.starts_with('#') && value[1..].bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(value.to_owned())
    } else {
        None
    }
}

fn to_xterm(colors: &Value) -> JsonValue {
    let mut theme = Map::new();
    for (group, source, target) in [
        ("primary", "foreground", "foreground"),
        ("primary", "background", "background"),
        ("cursor", "cursor", "cursor"),
        ("cursor", "text", "cursorAccent"),
        ("selection", "background", "selectionBackground"),
    ] {
        if let Some(value) = color(colors, group, source) {
            theme.insert(target.into(), JsonValue::String(value));
        }
    }
    for name in BASIC_COLORS {
        for (group, prefix) in [("normal", ""), ("bright", "bright"), ("dim", "dim")] {
            if let Some(value) = color(colors, group, name) {
                let key = if prefix.is_empty() { name.to_string() } else {
                    let mut chars = name.chars();
                    format!("{}{}{}", prefix, chars.next().unwrap().to_ascii_uppercase(), chars.as_str())
                };
                theme.insert(key, JsonValue::String(value));
            }
        }
    }
    JsonValue::Object(theme)
}

pub fn load_theme() -> JsonValue {
    let mut colors = Value::Table(Default::default());
    if let Some(path) = config_path() {
        read_colors(&path, &mut HashSet::new(), &mut colors);
    }
    to_xterm(&colors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_alacritty_color_tables() {
        let colors: Value = r##"
            [primary]
            foreground = "#abcdef"
            [normal]
            black = "#000000"
            [bright]
            red = "#ff0000"
            [cursor]
            cursor = "CellForeground"
        "##.parse().unwrap();
        let theme = to_xterm(&colors);
        assert_eq!(theme["foreground"], "#abcdef");
        assert_eq!(theme["black"], "#000000");
        assert_eq!(theme["brightRed"], "#ff0000");
        assert!(theme.get("cursor").is_none());
    }

    #[test]
    fn local_color_overrides_only_its_imported_key() {
        let mut imported: Value = "[primary]\nforeground = '#111111'\nbackground = '#222222'".parse().unwrap();
        let local: Value = "[primary]\nforeground = '#eeeeee'".parse().unwrap();
        merge(&mut imported, &local);
        assert_eq!(to_xterm(&imported)["foreground"], "#eeeeee");
        assert_eq!(to_xterm(&imported)["background"], "#222222");
    }

    #[test]
    fn imports_are_loaded_before_local_colors() {
        let mut colors = Value::Table(Default::default());
        let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/alacritty.toml");
        read_colors(&config, &mut HashSet::new(), &mut colors);
        let theme = to_xterm(&colors);
        assert_eq!(theme["foreground"], "#eeeeee");
        assert_eq!(theme["background"], "#222222");
        assert_eq!(theme["brightRed"], "#ff0000");
    }
}
