//! GTK terminal painting: matching background runs first, then Pango glyphs and cursor ink.
use crate::{graphics_ffi, viewport::Viewport};
use gtk::gdk::RGBA;
use worminal_config as theme;
use worminal_engine::Cell;
use worminal_engine_sys::*;

pub struct Palette {
    colors: Vec<RGBA>,
}
impl Default for Palette {
    fn default() -> Self {
        let mut palette = Self {
            colors: vec![RGBA::new(0.0, 0.0, 0.0, 0.0); theme::COLOR_NAMES.len()],
        };
        for index in 0..palette.colors.len() {
            palette.set(index, None);
        }
        palette
    }
}
impl Palette {
    pub fn set(&mut self, index: usize, value: Option<&str>) -> bool {
        let Some(current) = self.colors.get(index) else {
            return false;
        };
        let color = value
            .and_then(|value| RGBA::parse(value).ok())
            .unwrap_or_else(|| {
                if (16..232).contains(&index) {
                    let n = index - 16;
                    let component = |x: usize| {
                        if x == 0 {
                            0.0
                        } else {
                            (55 + 40 * x) as f64 / 255.0
                        }
                    };
                    RGBA::new(
                        component(n / 36),
                        component(n / 6 % 6),
                        component(n % 6),
                        1.0,
                    )
                } else if (232..256).contains(&index) {
                    let gray = (8 + 10 * (index - 232)) as f64 / 255.0;
                    RGBA::new(gray, gray, gray, 1.0)
                } else {
                    theme::COLOR_NAMES[index]
                        .and_then(|name| RGBA::parse(name).ok())
                        .unwrap_or(*current)
                }
            });
        let changed = *current != color;
        self.colors[index] = color;
        changed
    }
    pub fn color(&self, value: u32) -> RGBA {
        if value & WORMINAL_TRUECOLOR_FLAG != 0 {
            RGBA::new(
                ((value >> 16) & 255) as f64 / 255.0,
                ((value >> 8) & 255) as f64 / 255.0,
                (value & 255) as f64 / 255.0,
                1.0,
            )
        } else {
            self.colors
                .get(value as usize)
                .copied()
                .unwrap_or(self.colors[WORMINAL_DEFAULT_FOREGROUND as usize])
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metrics {
    pub width: i32,
    pub height: i32,
    pub ink_height: i32,
    pub ascent: i32,
}
#[derive(Clone)]
pub struct Font {
    description: pango::FontDescription,
    pub metrics: Metrics,
}
impl Font {
    pub fn apply(&self, layout: &pango::Layout) {
        layout.set_font_description(Some(&self.description));
    }
    pub fn resolve(layout: &pango::Layout, pattern: Option<&str>, zoom: f64) -> Self {
        let context = layout.context();
        graphics_ffi::resolution(&context, 96.0);
        let pattern = pattern.and_then(graphics_ffi::font_pattern);
        let family = pattern
            .as_ref()
            .and_then(|pattern| pattern.family.as_deref())
            .or(theme::FONT_FAMILY);
        let size = pattern
            .as_ref()
            .and_then(|pattern| pattern.size)
            .unwrap_or(theme::FONT_SIZE);
        let size = (size + zoom).max(4.0);
        let mut description = pango::FontDescription::new();
        // A missing compiled family inherits Pango's widget font, just as set_family(NULL) does in C.
        if let Some(family) = family {
            description.set_family(family);
        }
        if pattern.as_ref().is_some_and(|pattern| pattern.pixels) {
            description.set_absolute_size(size * f64::from(pango::SCALE));
        } else {
            description.set_size((size * f64::from(pango::SCALE)) as i32);
        }
        layout.set_font_description(Some(&description));
        layout.set_text(concat!(
            " !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ",
            "[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~"
        ));
        let width = (f64::from(layout.size().0) / (95.0 * f64::from(pango::SCALE))).ceil() as i32;
        let metrics = context.metrics(Some(&description), context.language().as_ref());
        let ceil = |value: i32| (value + pango::SCALE - 1) / pango::SCALE;
        let ascent = ceil(metrics.ascent());
        let ink_height = ascent + ceil(metrics.descent());
        Self {
            description,
            metrics: Metrics {
                width: width.max(1),
                height: (ink_height + theme::FONT_OFFSET_Y).max(1),
                ink_height,
                ascent,
            },
        }
    }
}

pub struct Style<'a> {
    pub palette: &'a Palette,
    pub mode: u32,
    pub cursor: u32,
    pub blink: bool,
    pub focused: bool,
    pub preedit: &'a str,
}
pub struct Painter {
    layout: pango::Layout,
    font: Font,
    ascii: Vec<Option<pango::Layout>>,
}
fn source(cr: &cairo::Context, color: &RGBA) {
    cr.set_source_rgba(color.red(), color.green(), color.blue(), color.alpha());
}
impl Painter {
    pub fn new(layout: pango::Layout, font: Font) -> Self {
        Self {
            layout,
            font,
            ascii: vec![None; 4 * 95],
        }
    }
    pub fn font(&self) -> &Font {
        &self.font
    }
    pub fn set_font(&mut self, font: Font) {
        self.font = font;
        self.ascii.fill(None);
    }
    pub fn overlap(&self) -> i32 {
        self.font.metrics.ink_height + theme::GLYPH_OFFSET_Y.saturating_abs()
    }
    pub fn damage(&self, row: usize, width: i32) -> (i32, i32, i32, i32) {
        let overlap = self.overlap();
        (
            0,
            2 + row as i32 * self.font.metrics.height - overlap,
            width,
            self.font.metrics.height + 2 * overlap,
        )
    }
    fn colors(
        &self,
        viewport: &Viewport,
        style: &Style<'_>,
        mut cell: Cell,
        x: usize,
        y: usize,
    ) -> (RGBA, RGBA, bool) {
        if theme::BOLD_BRIGHT != 0 && cell.attributes & ATTR_BOLD != 0 && cell.foreground < 8 {
            cell.foreground += 8;
        }
        let mut fg = style.palette.color(cell.foreground);
        let mut bg = style.palette.color(cell.background);
        let reverse = (cell.attributes & ATTR_REVERSE != 0)
            ^ (style.mode & MODE_REVERSE != 0)
            ^ viewport.selected(x, y);
        if reverse {
            std::mem::swap(&mut fg, &mut bg);
        }
        if cell.attributes & ATTR_FAINT != 0 {
            fg.set_red(fg.red() * 0.66);
            fg.set_green(fg.green() * 0.66);
            fg.set_blue(fg.blue() * 0.66);
        }
        if cell.attributes & ATTR_INVISIBLE != 0 || cell.attributes & ATTR_BLINK != 0 && style.blink
        {
            fg = bg;
        }
        let frame = viewport.frame();
        let cursor_x = frame.cursor_x as usize;
        let cursor_cell = x == cursor_x
            || x == cursor_x + 1
                && viewport
                    .row(y)
                    .is_some_and(|row| row[cursor_x].attributes & ATTR_WIDE != 0);
        let cursor = frame.scroll == 0
            && cursor_cell
            && y == frame.cursor_y as usize
            && style.mode & MODE_HIDE == 0
            && (!style.blink || style.cursor & 1 == 0)
            && viewport.live();
        let block = cursor && style.cursor <= 2 && style.focused;
        if block {
            bg = style.palette.color(WORMINAL_DEFAULT_CURSOR);
            fg = style.palette.color(WORMINAL_DEFAULT_BACKGROUND);
        }
        (fg, bg, cursor && !block)
    }
    pub fn text(&mut self, cr: &cairo::Context, value: &str, x: usize, y: usize, attributes: u32) {
        let style = usize::from(attributes & ATTR_BOLD != 0)
            | (usize::from(attributes & ATTR_ITALIC != 0) << 1);
        let cached = (value.len() == 1 && (b' '..=b'~').contains(&value.as_bytes()[0]))
            .then(|| style * 95 + usize::from(value.as_bytes()[0] - b' '));
        let existing = cached.and_then(|index| self.ascii[index].clone());
        let layout = if let Some(layout) = existing {
            layout
        } else {
            let layout = if cached.is_some() {
                pango::Layout::new(&self.layout.context())
            } else {
                self.layout.clone()
            };
            let mut font = self.font.description.clone();
            font.set_weight(if attributes & ATTR_BOLD != 0 {
                pango::Weight::Bold
            } else {
                pango::Weight::Normal
            });
            font.set_style(if attributes & ATTR_ITALIC != 0 {
                pango::Style::Italic
            } else {
                pango::Style::Normal
            });
            layout.set_font_description(Some(&font));
            layout.set_text(value);
            if let Some(index) = cached {
                self.ascii[index] = Some(layout.clone());
            }
            layout
        };
        let metrics = self.font.metrics;
        let top = 2 + (y as i32 + 1) * metrics.height - metrics.ink_height - theme::GLYPH_OFFSET_Y;
        if cached.is_some() || layout.line_count() == 1 {
            cr.move_to(
                f64::from(2 + x as i32 * metrics.width),
                f64::from(top) + f64::from(layout.baseline()) / f64::from(pango::SCALE),
            );
            if let Some(line) = layout.line_readonly(0) {
                graphics_ffi::show_line(cr, &line);
            }
        } else {
            cr.move_to(f64::from(2 + x as i32 * metrics.width), f64::from(top));
            graphics_ffi::show_layout(cr, &layout);
        }
    }
    fn background(
        &self,
        cr: &cairo::Context,
        color: RGBA,
        first: usize,
        end: usize,
        row: usize,
    ) -> Result<(), cairo::Error> {
        let metrics = self.font.metrics;
        source(cr, &color);
        cr.rectangle(
            f64::from(2 + first as i32 * metrics.width),
            f64::from(2 + row as i32 * metrics.height),
            (end - first) as f64 * f64::from(metrics.width),
            f64::from(metrics.height),
        );
        cr.fill()
    }
    fn backgrounds(
        &self,
        viewport: &Viewport,
        style: &Style<'_>,
        cr: &cairo::Context,
        first: usize,
        end: usize,
    ) -> Result<(), cairo::Error> {
        let matrix = cr.matrix();
        let aligned = |value: f64| value == value.floor();
        // Fractional device grids must retain scalar alpha edge blends, as in the C painter.
        let join = matrix.xy() == 0.0
            && matrix.yx() == 0.0
            && aligned(2.0 * matrix.xx() + matrix.x0())
            && aligned(2.0 * matrix.yy() + matrix.y0())
            && aligned(f64::from(self.font.metrics.width) * matrix.xx())
            && aligned(f64::from(self.font.metrics.height) * matrix.yy());
        for y in first..end {
            let Some(row) = viewport.row(y) else {
                continue;
            };
            let (_, mut color, _) = self.colors(viewport, style, row[0], 0, y);
            let mut start = 0;
            for (x, cell) in row.iter().enumerate().skip(1) {
                let (_, next, _) = self.colors(viewport, style, *cell, x, y);
                if join && next == color {
                    continue;
                }
                self.background(cr, color, start, x, y)?;
                start = x;
                color = next;
            }
            self.background(cr, color, start, row.len(), y)?;
        }
        Ok(())
    }
    pub fn paint(
        &mut self,
        viewport: &Viewport,
        style: &Style<'_>,
        cr: &cairo::Context,
    ) -> Result<(), cairo::Error> {
        source(
            cr,
            &style.palette.color(if style.mode & MODE_REVERSE != 0 {
                WORMINAL_DEFAULT_FOREGROUND
            } else {
                WORMINAL_DEFAULT_BACKGROUND
            }),
        );
        cr.paint()?;
        let (_, y1, _, y2) = cr.clip_extents()?;
        let metrics = self.font.metrics;
        let overlap = f64::from(self.overlap() + metrics.height);
        let first = ((y1 - 2.0 - overlap) / f64::from(metrics.height))
            .floor()
            .max(0.0) as usize;
        let end = (((y2 - 2.0 + overlap) / f64::from(metrics.height))
            .ceil()
            .max(0.0) as usize)
            .min(viewport.frame().rows as usize);
        self.backgrounds(viewport, style, cr, first, end)?;
        for y in first..end {
            let Some(row) = viewport.row(y) else {
                continue;
            };
            for (x, cell) in row.iter().enumerate() {
                if cell.attributes & ATTR_WDUMMY != 0 {
                    continue;
                }
                let blank = cell.rune == u32::from(' ')
                    && cell.attributes & (ATTR_UNDERLINE | ATTR_STRUCK) == 0;
                if blank && y != viewport.frame().cursor_y as usize {
                    continue;
                }
                let (fg, _, cursor) = self.colors(viewport, style, *cell, x, y);
                if blank && !cursor {
                    continue;
                }
                source(cr, &fg);
                if cell.rune != u32::from(' ') {
                    self.text(
                        cr,
                        char::from_u32(cell.rune)
                            .unwrap_or('\u{fffd}')
                            .encode_utf8(&mut [0; 4]),
                        x,
                        y,
                        cell.attributes,
                    );
                }
                let width = metrics.width
                    * if cell.attributes & ATTR_WIDE != 0 {
                        2
                    } else {
                        1
                    };
                if cell.attributes & ATTR_UNDERLINE != 0 {
                    cr.rectangle(
                        f64::from(2 + x as i32 * metrics.width),
                        f64::from(2 + (y as i32 + 1) * metrics.height - 1),
                        f64::from(width),
                        1.0,
                    );
                    cr.fill()?;
                }
                if cell.attributes & ATTR_STRUCK != 0 {
                    cr.rectangle(
                        f64::from(2 + x as i32 * metrics.width),
                        f64::from(2 + y as i32 * metrics.height + metrics.height / 2),
                        f64::from(width),
                        1.0,
                    );
                    cr.fill()?;
                }
                if cursor {
                    source(cr, &style.palette.color(WORMINAL_DEFAULT_CURSOR));
                    match style.cursor {
                        0..=2 => {
                            cr.set_line_width(1.0);
                            cr.rectangle(
                                2.5 + x as f64 * f64::from(metrics.width),
                                2.5 + y as f64 * f64::from(metrics.height),
                                f64::from(width - 1),
                                f64::from(metrics.height - 1),
                            );
                            cr.stroke()?;
                        }
                        7 => self.text(cr, "☃", x, y, 0),
                        5 | 6 => cr.rectangle(
                            f64::from(2 + x as i32 * metrics.width),
                            f64::from(2 + y as i32 * metrics.height),
                            2.0,
                            f64::from(metrics.height),
                        ),
                        _ => cr.rectangle(
                            f64::from(2 + x as i32 * metrics.width),
                            f64::from((y as i32 + 1) * metrics.height),
                            f64::from(metrics.width),
                            2.0,
                        ),
                    }
                    cr.fill()?;
                }
            }
        }
        if !style.preedit.is_empty() {
            source(cr, &style.palette.color(WORMINAL_DEFAULT_FOREGROUND));
            self.text(
                cr,
                style.preedit,
                viewport.frame().cursor_x as usize,
                viewport.frame().cursor_y as usize,
                ATTR_UNDERLINE,
            );
        }
        Ok(())
    }
}
