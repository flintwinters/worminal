//! Self-describing configuration: `worminald config --help` and `--example` are generated here, so an agent
//! holding only the executable can edit someone's configuration correctly. `unknown_keys` uses the same table
//! to flag settings Worminal ignores; tests tie the table, the example and the resolver together.
use serde_json::Value;

/// `<name>` stands for any of black red green yellow blue magenta cyan white.
const KEYS: &[(&str, &str, &str)] = &[
    ("font.size", "number > 0 (points)", "9"),
    (
        "font.normal.family",
        "font family name",
        "Atkinson Hyperlegible Mono",
    ),
    ("font.offset.y", "integer (pixels of line spacing)", "-3"),
    ("font.glyph_offset.y", "integer (pixels)", "0"),
    ("scrolling.history", "integer 0..=100000 (lines)", "10000"),
    (
        "scrolling.multiplier",
        "integer >= 1 (lines per wheel step)",
        "3",
    ),
    ("colors.primary.foreground", "color", "#d8d8d8"),
    ("colors.primary.background", "color", "#181818"),
    (
        "colors.primary.bright_foreground",
        "color",
        "the foreground",
    ),
    (
        "colors.primary.dim_foreground",
        "color",
        "66% of the foreground",
    ),
    ("colors.normal.<name>", "color", "see --example"),
    ("colors.bright.<name>", "color", "see --example"),
    (
        "colors.dim.<name>",
        "color",
        "66% of normal, or fixed grays if a dim table exists",
    ),
    (
        "colors.cursor.cursor",
        "color | CellForeground | CellBackground",
        "CellForeground",
    ),
    (
        "colors.cursor.text",
        "color | CellForeground | CellBackground",
        "CellBackground",
    ),
    (
        "colors.indexed_colors",
        "list of {index: 16..=255, color: color}",
        "none (synthesized)",
    ),
    (
        "colors.draw_bold_text_with_bright_colors",
        "boolean",
        "false",
    ),
    (
        "draw_bold_text_with_bright_colors",
        "boolean (legacy top-level spelling)",
        "false",
    ),
    ("worminal.update_notifications", "boolean", "false"),
    ("worminal.group_tabs_by_title", "boolean", "true"),
    (
        "general.import",
        "list of file paths (Alacritty import)",
        "none",
    ),
    ("import", "list of file paths (legacy spelling)", "none"),
];

const COLOR_NAMES: [&str; 8] = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
];

/// A complete worminal.yaml spelling out the defaults; tests keep it equal to the built-in values.
pub const EXAMPLE: &str = "\
# worminal.yaml: only the differences from the defaults are needed. Everything below is the default.
font:
  size: 9
  offset: {y: -3}
  glyph_offset: {y: 0}
  normal: {family: Atkinson Hyperlegible Mono}
scrolling:
  history: 10000
  multiplier: 3
colors:
  draw_bold_text_with_bright_colors: false
  primary: {foreground: '#d8d8d8', background: '#181818'}
  cursor: {cursor: CellForeground, text: CellBackground}
  normal:
    {black: '#181818', red: '#ac4242', green: '#90a959', yellow: '#f4bf75', blue: '#6a9fb5', magenta: '#aa759f', cyan: '#75b5aa', white: '#d8d8d8'}
  bright:
    {black: '#6b6b6b', red: '#c55555', green: '#aac474', yellow: '#feca88', blue: '#82b8c8', magenta: '#c28cb8', cyan: '#93d3c3', white: '#f8f8f8'}
worminal:
  update_notifications: false
  group_tabs_by_title: true
";

fn matches(pattern: &str, path: &[String]) -> bool {
    let parts: Vec<&str> = pattern.split('.').collect();
    parts.len() == path.len()
        && parts.iter().zip(path).all(|(part, segment)| {
            *part == segment || (*part == "<name>" && COLOR_NAMES.contains(&segment.as_str()))
        })
}

fn is_prefix(pattern: &str, path: &[String]) -> bool {
    let parts: Vec<&str> = pattern.split('.').collect();
    parts.len() > path.len()
        && parts.iter().zip(path).all(|(part, segment)| {
            *part == segment || (*part == "<name>" && COLOR_NAMES.contains(&segment.as_str()))
        })
}

/// Dotted paths in one configuration file that Worminal does not read (typos, or Alacritty settings with no
/// Worminal equivalent). They are ignored, never errors: Alacritty configs legitimately carry many more keys.
pub fn unknown_keys(config: &Value) -> Vec<String> {
    fn walk(value: &Value, path: &mut Vec<String>, found: &mut Vec<String>) {
        let Some(table) = value.as_object() else {
            return;
        };
        for (key, child) in table {
            path.push(key.clone());
            if KEYS.iter().any(|(known, ..)| matches(known, path)) {
                // A leaf: its value is validated by the resolver.
            } else if KEYS.iter().any(|(known, ..)| is_prefix(known, path)) {
                walk(child, path, found);
            } else {
                found.push(path.join("."));
            }
            path.pop();
        }
    }
    let mut found = Vec::new();
    walk(config, &mut Vec::new(), &mut found);
    found
}

fn keys_text() -> String {
    KEYS.iter()
        .map(|(path, kind, default)| format!("  {path:<42} {kind}; default: {default}\n"))
        .collect()
}

/// The complete reference printed by `worminald config --help`.
pub fn help() -> String {
    format!(
        "worminald config: inspect, validate and describe the runtime configuration.

Commands (none start a daemon):
  worminald config                     validate strictly: 'configuration is valid' (exit 0) or the reason (exit 1);
                                       also lists settings in worminal.yaml that Worminal ignores
  worminald config --paths             files in use, and where worminal.yaml belongs if it does not exist yet
  worminald config --json [--defaults] resolved settings as JSON (--defaults ignores every file)
  worminald config --example           a complete worminal.yaml showing every default; edit a copy
  worminald config sync [--prefer local|remote] SSH_ENDPOINT
                                       merge worminal.yaml with a peer per key; conflicts stay on both sides and are
                                       reported until --prefer picks one; comments in rewritten files are lost

Files, lowest to highest priority:
  1. An existing Alacritty config (alacritty.toml, or legacy alacritty.yml/.yaml, with `import`), found in
     Alacritty's search order, or the file named by WORMINAL_ALACRITTY_CONFIG.
  2. worminal.yaml: $XDG_CONFIG_HOME/worminal/worminal.yaml (default ~/.config/worminal/worminal.yaml), or the
     file named by WORMINAL_CONFIG (which must exist).
Layering is per key: worminal.yaml needs only the differences, and a working Alacritty setup keeps working.
Alacritty keys Worminal does not use (window, keyboard, ...) are ignored. Every other source of settings is
absent: nothing is compiled in, and each process reads the files itself, also on headless hosts.

Settings (YAML; colors are '#rrggbb' or 0xrrggbb; quote '#' values):
{keys}
Color slots in `--json`: colors[0..8] normal, [8..16] bright, [16..256] indexed (null = synthesized),
256 cursor, 257 cursor text, 258 foreground, 259 background, 260 bright foreground, 261..=268 dim, 269 dim foreground.

To configure for someone:
  1. worminald config --paths           find the file; create it (and its directory) if it does not exist
  2. Read it, add or change only the requested keys, keep their other settings and comments
  3. worminald config                   must say 'configuration is valid'; fix the reported key if not
  4. worminald config --json            confirm the resolved values are what was meant
Invalid configuration never stops Worminal: it reports to stderr and runs with defaults, so always validate.

When it applies: each process reads configuration once, at start. New windows pick up appearance and
update_notifications. scrolling.history and worminal.group_tabs_by_title are the daemon's: they take effect when
the daemon next restarts (existing shells are preserved by the normal handoff). Running windows never change.
Other devices: `worminald config sync` (views also run it once per host on connect). Only worminal.yaml syncs.",
        keys = keys_text()
    )
}
