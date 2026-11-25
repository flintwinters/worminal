//! Terminal palette resolution shared by graphical and VT presenters.
use crate::Settings;

pub fn rgb_hex(color: &str) -> Option<u32> {
    let digits = color.strip_prefix('#')?;
    match digits.len() {
        6 => u32::from_str_radix(digits, 16).ok(),
        3 => {
            let packed = u32::from_str_radix(digits, 16).ok()?;
            Some(((packed & 0xf00) << 8 | (packed & 0xf0) << 4 | packed & 0xf) * 17)
        }
        _ => None,
    }
}

pub fn palette_rgb(settings: &Settings, index: usize) -> Option<u32> {
    let rgb = |red, green, blue| (red << 16) | (green << 8) | blue;
    match index {
        16..=231 => {
            let n = index - 16;
            let component = |x| if x == 0 { 0 } else { 55 + 40 * x } as u32;
            Some(rgb(
                component(n / 36),
                component(n / 6 % 6),
                component(n % 6),
            ))
        }
        232..=255 => {
            let gray = (8 + 10 * (index - 232)) as u32;
            Some(rgb(gray, gray, gray))
        }
        _ => rgb_hex(settings.colors.get(index)?.as_deref()?),
    }
}
