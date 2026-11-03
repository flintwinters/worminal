//! The one-cell Cairo strip shares terminal fonts/colors. Drawing and hit testing share the visible suffix.
use crate::{
    fonts::Fonts,
    graphics_ffi,
    paint::{self, Metrics, Palette},
};
use gtk::{gdk, prelude::*};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use worminal_engine_sys::{WORMINAL_DEFAULT_BACKGROUND as BG, WORMINAL_DEFAULT_FOREGROUND as FG};

#[derive(Clone)]
pub struct Tab {
    pub id: u32,
    pub label: String,
    pub browser: bool,
}
struct State {
    widget: gtk::DrawingArea,
    fonts: Rc<RefCell<Fonts>>,
    layout: RefCell<Option<pango::Layout>>,
    metrics: Cell<Option<Metrics>>,
    entries: RefCell<Vec<Tab>>,
    active: Cell<u32>,
    palette: RefCell<Option<Rc<RefCell<Palette>>>>,
}
#[derive(Clone)]
pub struct Tabs(Rc<State>);

fn first_visible(
    count: usize,
    active: Option<usize>,
    right: i32,
    mut width: impl FnMut(usize) -> i32,
) -> Option<usize> {
    let mut first = active.or_else(|| count.checked_sub(1))?;
    let mut total = 2 + width(first);
    while first > 0 && total <= right {
        let previous = width(first - 1);
        if previous > right - total {
            break;
        }
        total += previous;
        first -= 1;
    }
    Some(first)
}
fn width(layout: &pango::Layout, entry: &Tab, metrics: Metrics) -> i32 {
    layout.set_text(&entry.label);
    layout.pixel_size().0 + 2 * metrics.width
}
impl State {
    fn draw(&self, cr: &cairo::Context, right: i32) -> Result<(), cairo::Error> {
        let Some(metrics) = self.metrics.get() else {
            return Ok(());
        };
        let layout = self.layout.borrow();
        let Some(layout) = layout.as_ref() else {
            return Ok(());
        };
        let palette = self.palette.borrow();
        let Some(palette) = palette.as_ref() else {
            return Ok(());
        };
        let palette = palette.borrow();
        let entries = self.entries.borrow();
        let active = entries
            .iter()
            .position(|entry| entry.id == self.active.get());
        let Some(first) = first_visible(entries.len(), active, right, |i| {
            width(layout, &entries[i], metrics)
        }) else {
            return Ok(());
        };
        paint::source(cr, &palette.color(BG));
        cr.paint()?;
        cr.rectangle(
            2.0,
            2.0,
            f64::from((right - 2).max(0)),
            f64::from(metrics.height),
        );
        cr.clip();
        let baseline = 2 + (metrics.height - metrics.ink_height) / 2 + metrics.ascent;
        let mut x = 2;
        for entry in &entries[first..] {
            if x >= right {
                break;
            }
            let width = width(layout, entry, metrics);
            let selected = entry.id == self.active.get();
            if selected {
                paint::source(cr, &palette.color(FG));
                cr.rectangle(
                    f64::from(x),
                    2.0,
                    f64::from(width.min(right - x)),
                    f64::from(metrics.height),
                );
                cr.fill()?;
            }
            paint::source(
                cr,
                &palette.color(if selected {
                    BG
                } else if entry.browser {
                    1
                } else {
                    7
                }),
            );
            cr.move_to(
                f64::from(x + metrics.width),
                f64::from(baseline) - f64::from(layout.baseline()) / f64::from(pango::SCALE),
            );
            graphics_ffi::show_layout(cr, layout);
            cr.rectangle(
                f64::from(x + metrics.width),
                f64::from((baseline + 2).min(metrics.height)),
                f64::from((width - 2 * metrics.width).max(0)),
                1.0,
            );
            cr.fill()?;
            x += width;
        }
        Ok(())
    }
    fn hit(&self, x: f64, y: f64) -> Option<u32> {
        let metrics = self.metrics.get()?;
        if y < 2.0 || y >= f64::from(2 + metrics.height) {
            return None;
        }
        let layout = self.layout.borrow();
        let layout = layout.as_ref()?;
        let entries = self.entries.borrow();
        let right = self.widget.allocated_width() - 2;
        let first = first_visible(
            entries.len(),
            entries
                .iter()
                .position(|entry| entry.id == self.active.get()),
            right,
            |i| width(layout, &entries[i], metrics),
        )?;
        let mut left = 2;
        for entry in &entries[first..] {
            if left >= right {
                break;
            }
            let width = width(layout, entry, metrics);
            if x >= f64::from(left) && x < f64::from((left + width).min(right)) {
                return Some(entry.id);
            }
            left += width;
        }
        None
    }
}
impl Tabs {
    pub fn new(fonts: Rc<RefCell<Fonts>>) -> Self {
        let state = Rc::new(State {
            widget: gtk::DrawingArea::new(),
            fonts,
            layout: RefCell::new(None),
            metrics: Cell::new(None),
            entries: RefCell::new(Vec::new()),
            active: Cell::new(0),
            palette: RefCell::new(None),
        });
        state.widget.add_events(gdk::EventMask::BUTTON_PRESS_MASK);
        state.widget.connect_draw(
            glib::clone!(@weak state => @default-return glib::Propagation::Proceed,
                move |widget, cr| {
                if let Err(error) = state.draw(cr, widget.allocated_width() - 2) {
                    eprintln!("worminal: tab strip: {error}");
                }
                glib::Propagation::Proceed
            }),
        );
        Self(state)
    }
    pub fn widget(&self) -> &gtk::DrawingArea {
        &self.0.widget
    }
    pub fn connect_selected(&self, select: impl Fn(u32) + 'static) {
        let state = self.0.clone();
        self.0.widget.connect_button_press_event(glib::clone!(@weak state
            => @default-return glib::Propagation::Proceed, move |_, event| {
                let (x, y) = event.position();
                if event.button() == 1 {
                    if let Some(id) = state.hit(x, y) { select(id); return glib::Propagation::Stop; }
                }
                glib::Propagation::Proceed
            }));
    }
    pub fn update(&self, entries: Vec<Tab>, active: u32, palette: Option<Rc<RefCell<Palette>>>) {
        self.0.active.set(active);
        *self.0.palette.borrow_mut() = palette;
        let empty = entries.is_empty();
        *self.0.entries.borrow_mut() = entries;
        if !empty {
            let mut layout = self.0.layout.borrow_mut();
            let layout = layout.get_or_insert_with(|| self.0.widget.create_pango_layout(None));
            let metrics = self.0.fonts.borrow_mut().resolve(layout).metrics;
            self.0.metrics.set(Some(metrics));
            self.0.widget.set_size_request(-1, metrics.height + 2);
        }
        self.0.widget.queue_draw();
    }
}

#[cfg(test)]
mod tests {
    use super::first_visible;
    #[test]
    fn visible_suffix_matches_independent_prefix_widths_and_clips_oversized_active_tab() {
        let widths: Vec<i32> = (0..64)
            .map(|i| if i == 31 { 3000 } else { 20 + i % 13 * 7 })
            .collect();
        for active in 0..64 {
            for right in [0, 40, 320, 800, 4096] {
                let mut first = 0;
                let mut total = 2 + widths[..=active].iter().sum::<i32>();
                while first < active && total > right {
                    total -= widths[first];
                    first += 1;
                }
                assert_eq!(
                    first_visible(widths.len(), Some(active), right, |i| widths[i]),
                    Some(first)
                );
            }
        }
        assert_eq!(first_visible(0, None, 320, |_| unreachable!()), None);
    }
}
