//! Validation of the layered configuration tree into the values processes consume.
use crate::source::merge;
use serde_json::{json, Value};

/// Indexed colors plus cursor, default and dim slots; must equal `WORMINAL_COLOR_COUNT` in the engine.
pub const COLOR_COUNT: usize = 270;
const NAMES: [&str; 8] = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
];
const DIM_DEFAULTS: [&str; 8] = [
    "#0f0f0f", "#712b2b", "#5f6f3a", "#a17e4d", "#456877", "#704d68", "#4d7770", "#8e8e8e",
];
const PALETTE: &str = include_str!("../../../assets/palette.json");
const FONT: &str = include_str!("../../../assets/font.yaml");

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub font_family: Option<String>,
    pub font_size: f64,
    pub font_offset_y: i32,
    pub glyph_offset_y: i32,
    pub history_size: i32,
    pub scroll_multiplier: i32,
    pub bold_bright: bool,
    pub update_notifications: bool,
    /// The daemon moves a tab beside another with the same title when its title changes.
    pub group_tabs_by_title: bool,
    /// `#rrggbb` per palette slot. Unset indexed slots stay `None`: renderers synthesize them.
    pub colors: Vec<Option<String>>,
}

type Fallible<T> = Result<T, String>;

fn table<'a>(parent: &'a Value, key: &str, name: &str) -> Fallible<&'a Value> {
    static EMPTY: Value = Value::Null;
    match parent.get(key) {
        None => Ok(&EMPTY),
        Some(value) if value.is_object() => Ok(value),
        Some(_) => Err(format!("{name} must be a mapping")),
    }
}

fn integer(
    value: Option<&Value>,
    default: i64,
    name: &str,
    range: std::ops::RangeInclusive<i64>,
) -> Fallible<i64> {
    let Some(value) = value else {
        return Ok(default);
    };
    value
        .as_i64()
        .filter(|number| range.contains(number))
        .ok_or_else(|| {
            format!(
                "{name} must be an integer from {} to {}",
                range.start(),
                range.end()
            )
        })
}

fn boolean(value: Option<&Value>, default: bool, name: &str) -> Fallible<bool> {
    match value {
        None => Ok(default),
        Some(Value::Bool(flag)) => Ok(*flag),
        Some(_) => Err(format!("{name} must be boolean")),
    }
}

/// `#rrggbb`, `0xrrggbb`, or the integer YAML makes of an unquoted `0xrrggbb`.
fn color(value: Option<&Value>, name: &str) -> Fallible<String> {
    let invalid = || format!("{name} must be a #RRGGBB or 0xRRGGBB color");
    let digits = match value {
        Some(Value::String(text)) => text
            .strip_prefix('#')
            .or_else(|| text.strip_prefix("0x"))
            .filter(|digits| {
                digits.len() == 6 && digits.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            .map(str::to_ascii_lowercase),
        Some(Value::Number(number)) => number
            .as_u64()
            .filter(|number| *number <= 0xff_ffff)
            .map(|number| format!("{number:06x}")),
        _ => None,
    };
    digits
        .map(|digits| format!("#{digits}"))
        .ok_or_else(invalid)
}

/// Alacritty's display/color.rs scales each component by 0.66 and truncates.
fn dim(color: &str) -> String {
    let mut result = String::from("#");
    for index in [1, 3, 5] {
        let component = u8::from_str_radix(&color[index..index + 2], 16).unwrap_or(0);
        result.push_str(&format!("{:02x}", (f64::from(component) * 0.66) as u8));
    }
    result
}

/// All 270 palette slots from the `colors` table, with Alacritty's defaults and derivations.
fn palette(config: &Value) -> Fallible<Vec<Option<String>>> {
    let palette: Value = serde_json::from_str(PALETTE).map_err(|error| error.to_string())?;
    let colors = table(config, "colors", "colors")?;
    let groups = ["primary", "normal", "bright", "dim", "cursor"]
        .map(|group| table(colors, group, &format!("colors.{group}")));
    let [primary, normal_colors, bright, dim_table, cursor] = groups;
    let (primary, normal_colors, bright, dim_table, cursor) =
        (primary?, normal_colors?, bright?, dim_table?, cursor?);

    let mut slots: Vec<Option<String>> = vec![None; COLOR_COUNT];
    for (group, table, start) in [("normal", normal_colors, 0), ("bright", bright, 8)] {
        for (index, name) in NAMES.iter().enumerate() {
            let value = table.get(name).or(palette[group].get(name));
            slots[start + index] = Some(color(value, &format!("colors.{group}.{name}"))?);
        }
    }
    match colors.get("indexed_colors") {
        None => {}
        Some(Value::Array(entries)) => {
            for entry in entries {
                let index = entry
                    .get("index")
                    .and_then(Value::as_i64)
                    .ok_or("indexed_colors entries need an integer index")?;
                if !(16..=255).contains(&index) {
                    return Err("indexed_colors indices must be 16 through 255".into());
                }
                slots[index as usize] = Some(color(
                    entry.get("color"),
                    &format!("indexed_colors[{index}].color"),
                )?);
            }
        }
        Some(_) => return Err("colors.indexed_colors must be a list".into()),
    }
    let foreground = color(
        primary
            .get("foreground")
            .or(palette["primary"].get("foreground")),
        "colors.primary.foreground",
    )?;
    let background = color(
        primary
            .get("background")
            .or(palette["primary"].get("background")),
        "colors.primary.background",
    )?;
    let cursor_color = |key: &str, default: &str| -> Fallible<String> {
        match cursor.get(key).unwrap_or(&json!(default)) {
            Value::String(text) if text == "CellForeground" => Ok(foreground.clone()),
            Value::String(text) if text == "CellBackground" => Ok(background.clone()),
            other => color(Some(other), &format!("colors.cursor.{key}")),
        }
    };
    slots[256] = Some(cursor_color("cursor", "CellForeground")?);
    slots[257] = Some(cursor_color("text", "CellBackground")?);
    slots[258] = Some(foreground.clone());
    slots[259] = Some(background);
    slots[260] = Some(match primary.get("bright_foreground") {
        None => foreground.clone(),
        value => color(value, "colors.primary.bright_foreground")?,
    });
    // With a dim table, Alacritty uses its fixed defaults for missing fields; without one it derives
    // the whole table from the normal colors.
    let has_dim = dim_table.as_object().is_some_and(|table| !table.is_empty());
    for (index, name) in NAMES.iter().enumerate() {
        let fallback = if has_dim {
            DIM_DEFAULTS[index].to_string()
        } else {
            dim(slots[index].as_deref().unwrap_or("#000000"))
        };
        slots[261 + index] = Some(color(
            Some(dim_table.get(name).unwrap_or(&json!(fallback))),
            &format!("colors.dim.{name}"),
        )?);
    }
    slots[269] = Some(color(
        Some(
            primary
                .get("dim_foreground")
                .unwrap_or(&json!(dim(&foreground))),
        ),
        "colors.primary.dim_foreground",
    )?);
    Ok(slots)
}

impl Settings {
    pub fn resolve(config: &Value) -> Fallible<Self> {
        let config = if config.is_null() { &json!({}) } else { config };
        let mut font = serde_yaml_ng::from_str::<Value>(FONT).map_err(|error| error.to_string())?
            ["font"]
            .clone();
        let overrides = table(config, "font", "font")?;
        if overrides.is_object() {
            merge(&mut font, overrides);
        }
        let normal = table(&font, "normal", "font.normal")?;
        let family = match normal.get("family") {
            None | Some(Value::Null) => None,
            Some(Value::String(text))
                if !text.trim().is_empty() && !text.chars().any(|c| (c as u32) < 32) =>
            {
                Some(text.clone())
            }
            Some(_) => return Err("font.normal.family must be a nonempty font family".into()),
        };
        let offset = table(&font, "offset", "font.offset")?;
        let glyph = table(&font, "glyph_offset", "font.glyph_offset")?;
        let signed = i64::from(i32::MIN)..=i64::from(i32::MAX);
        let size = match font.get("size") {
            None => 0.0,
            Some(Value::Number(number)) => number
                .as_f64()
                .filter(|size| size.is_finite() && *size > 0.0)
                .ok_or("font.size must be a positive number")?,
            Some(_) => return Err("font.size must be a positive number".into()),
        };
        let scrolling = table(config, "scrolling", "scrolling")?;
        let colors = table(config, "colors", "colors")?;

        let bold = colors
            .get("draw_bold_text_with_bright_colors")
            .or(config.get("draw_bold_text_with_bright_colors"));
        let worminal = table(config, "worminal", "worminal")?;
        Ok(Self {
            font_family: family,
            font_size: size,
            font_offset_y: integer(offset.get("y"), 0, "font.offset.y", signed.clone())? as i32,
            glyph_offset_y: integer(glyph.get("y"), 0, "font.glyph_offset.y", signed)? as i32,
            history_size: integer(
                scrolling.get("history"),
                10000,
                "scrolling.history",
                0..=100_000,
            )? as i32,
            scroll_multiplier: integer(
                scrolling.get("multiplier"),
                3,
                "scrolling.multiplier",
                1..=i64::from(i32::MAX),
            )? as i32,
            bold_bright: boolean(bold, false, "colors.draw_bold_text_with_bright_colors")?,
            update_notifications: boolean(
                worminal.get("update_notifications"),
                false,
                "worminal.update_notifications",
            )?,
            group_tabs_by_title: boolean(
                worminal.get("group_tabs_by_title"),
                true,
                "worminal.group_tabs_by_title",
            )?,
            colors: palette(config)?,
        })
    }

    /// Stable machine-readable form for tools outside Rust (explorer font, historical C reference).
    pub fn to_json(&self) -> Value {
        json!({
            "font_family": self.font_family,
            "font_size": self.font_size,
            "font_offset_y": self.font_offset_y,
            "glyph_offset_y": self.glyph_offset_y,
            "history_size": self.history_size,
            "scroll_multiplier": self.scroll_multiplier,
            "bold_bright": self.bold_bright,
            "update_notifications": self.update_notifications,
            "group_tabs_by_title": self.group_tabs_by_title,
            "colors": self.colors,
        })
    }
}
