//! The one-cell Cairo strip shares terminal fonts/colors. Tabs wrap onto further rows in daemon order; drawing and hit testing share the row placement.
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
use worminal_protocol::{ATTENTION_DONE, ATTENTION_FAILED, ATTENTION_HELP};

#[derive(Clone)]
pub struct Tab {
    pub id: u32,
    pub label: String,
    pub browser: bool,
    /// Shown by a daemon on another host.
    pub remote: bool,
    /// An SSH client is in the terminal's foreground.
    pub ssh: bool,
}
/// A primary-button press on a tab. It becomes a reorder once the pointer travels past `DRAG_SLOP`.
struct Drag {
    id: u32,
    x: f64,
    y: f64,
    active: bool,
    /// The tab whose slot the dragged tab takes if released now.
    over: Option<u32>,
}
const DRAG_SLOP: f64 = 6.0;
struct State {
    widget: gtk::DrawingArea,
    fonts: Rc<RefCell<Fonts>>,
    layout: RefCell<Option<pango::Layout>>,
    metrics: Cell<Option<Metrics>>,
    entries: RefCell<Vec<Tab>>,
    active: Cell<u32>,
    palette: RefCell<Option<Rc<RefCell<Palette>>>>,
    /// Tabs (with severity) that rang the bell and have not received input since; selecting a tab keeps it.
    attention: RefCell<Vec<(u32, u32)>>,
    /// Bells rung by each tab in its latest burst, with the time of the last one.
    bells: RefCell<Vec<(u32, u32, std::time::Instant)>>,
    /// Attention glow strength, 0..=1. The timer runs only while some tab needs attention.
    glow: Cell<f64>,
    blink: RefCell<Option<glib::SourceId>>,
    drag: RefCell<Option<Drag>>,
}
#[derive(Clone)]
pub struct Tabs(Rc<State>);

/// Greedy wrap in tab order: `(row, x)` per tab. A tab wider than a row sits alone and is clipped.
fn place(widths: &[i32], right: i32) -> Vec<(i32, i32)> {
    let (mut row, mut x) = (0, 2);
    widths
        .iter()
        .map(|&width| {
            if x > 2 && x + width > right {
                row += 1;
                x = 2;
            }
            let at = (row, x);
            x += width;
            at
        })
        .collect()
}
/// Seconds per full fade out and back in.
/// ANSI magenta: tabs whose foreground is an SSH session.
const SSH: u32 = 5;
const GLOW_PERIOD: f64 = 2.0;
fn width(layout: &pango::Layout, entry: &Tab, metrics: Metrics) -> i32 {
    layout.set_text(&entry.label);
    layout.pixel_size().0 + 2 * metrics.width
}
fn mix(from: &gdk::RGBA, to: &gdk::RGBA, amount: f64) -> gdk::RGBA {
    let blend = |a: f64, b: f64| a + (b - a) * amount;
    gdk::RGBA::new(
        blend(from.red(), to.red()),
        blend(from.green(), to.green()),
        blend(from.blue(), to.blue()),
        1.0,
    )
}
/// Background fill (none for resting tabs) and text color of one tab at the given glow strength.
fn colors(
    palette: &Palette,
    entry: &Tab,
    selected: bool,
    glow: f64,
    severity: u32,
) -> (Option<gdk::RGBA>, gdk::RGBA) {
    let selected_fill = palette.color(if entry.ssh { SSH } else { FG });
    let fill = if glow > 0.0 {
        let from = if selected {
            selected_fill
        } else {
            palette.color(BG)
        };
        Some(mix(&from, &palette.color(severity_color(severity)), glow))
    } else {
        selected.then_some(selected_fill)
    };
    let resting = palette.color(if selected {
        BG
    } else if entry.ssh {
        SSH
    } else if entry.browser {
        1
    } else if entry.remote {
        2
    } else {
        7
    });
    (fill, mix(&resting, &palette.color(BG), glow))
}
/// Palette color of an alert: green when finished, yellow when the user is needed, red on failure.
fn severity_color(severity: u32) -> u32 {
    match severity {
        ATTENTION_DONE => 2,
        ATTENTION_HELP => 3,
        _ => 1,
    }
}
/// Bells closer together than this belong to one burst, whose length is the alert's severity.
const BELL_BURST: std::time::Duration = std::time::Duration::from_millis(1500);
/// Attention glow for `seconds` into an alert: starts at full strength, then breathes on a cosine.
fn glow_at(seconds: f64) -> f64 {
    (1.0 + (seconds * std::f64::consts::TAU / GLOW_PERIOD).cos()) / 2.0
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
        let widths: Vec<i32> = entries
            .iter()
            .map(|entry| width(layout, entry, metrics))
            .collect();
        paint::source(cr, &palette.color(BG));
        cr.paint()?;
        // The marker sits on the side the dragged tab approaches from.
        let drop = self.drag.borrow().as_ref().and_then(|drag| {
            let from = entries.iter().position(|entry| entry.id == drag.id)?;
            let to = entries
                .iter()
                .position(|entry| Some(entry.id) == drag.over)?;
            drag.active.then_some((to, to > from))
        });
        for (i, (entry, (row, x))) in entries.iter().zip(place(&widths, right)).enumerate() {
            let top = 2 + row * metrics.height;
            let width = widths[i];
            cr.save()?;
            cr.rectangle(
                2.0,
                f64::from(top),
                f64::from((right - 2).max(0)),
                f64::from(metrics.height),
            );
            cr.clip();
            // Measuring left the layout holding the last label.
            layout.set_text(&entry.label);
            let baseline = top + (metrics.height - metrics.ink_height) / 2 + metrics.ascent;
            let selected = entry.id == self.active.get();
            let severity = self
                .attention
                .borrow()
                .iter()
                .find(|(id, _)| *id == entry.id)
                .map_or(0, |(_, severity)| *severity);
            let glow = if severity > 0 { self.glow.get() } else { 0.0 };
            let (fill, text) = colors(&palette, entry, selected, glow, severity);
            if let Some(fill) = fill {
                paint::source(cr, &fill);
                cr.rectangle(
                    f64::from(x),
                    f64::from(top),
                    f64::from(width.min(right - x)),
                    f64::from(metrics.height),
                );
                cr.fill()?;
            }
            paint::source(cr, &text);
            cr.move_to(
                f64::from(x + metrics.width),
                f64::from(baseline) - f64::from(layout.baseline()) / f64::from(pango::SCALE),
            );
            graphics_ffi::show_layout(cr, layout);
            cr.rectangle(
                f64::from(x + metrics.width),
                f64::from((baseline + 2).min(top - 2 + metrics.height)),
                f64::from((width - 2 * metrics.width).max(0)),
                1.0,
            );
            cr.fill()?;
            if let Some((_, after)) = drop.filter(|(to, _)| *to == i) {
                paint::source(cr, &palette.color(3));
                let edge = if after { x + width - 2 } else { x };
                cr.rectangle(
                    f64::from(edge),
                    f64::from(top),
                    2.0,
                    f64::from(metrics.height),
                );
                cr.fill()?;
            }
            cr.restore()?;
        }
        Ok(())
    }
    fn hit(&self, x: f64, y: f64) -> Option<u32> {
        let metrics = self.metrics.get()?;
        let layout = self.layout.borrow();
        let layout = layout.as_ref()?;
        let entries = self.entries.borrow();
        let right = self.widget.allocated_width() - 2;
        let widths: Vec<i32> = entries
            .iter()
            .map(|entry| width(layout, entry, metrics))
            .collect();
        entries
            .iter()
            .zip(place(&widths, right))
            .zip(&widths)
            .find(|((_, (row, left)), width)| {
                let top = 2 + row * metrics.height;
                y >= f64::from(top)
                    && y < f64::from(top + metrics.height)
                    && x >= f64::from(*left)
                    && x < f64::from((left + **width).min(right))
            })
            .map(|((entry, _), _)| entry.id)
    }
    fn drag_to(&self, x: f64, y: f64) {
        let over = self.hit(x, y);
        let mut drag = self.drag.borrow_mut();
        let Some(drag) = drag.as_mut() else { return };
        if !drag.active && (x - drag.x).hypot(y - drag.y) < DRAG_SLOP {
            return;
        }
        // Keep the last target while the pointer crosses gaps.
        let over = over.filter(|id| *id != drag.id).or(drag.over);
        if !drag.active || over != drag.over {
            drag.active = true;
            drag.over = over;
            self.widget.queue_draw();
        }
    }
    /// Height follows the row count, which depends on the allocated width.
    fn fit(&self) {
        let (Some(metrics), Some(layout)) = (self.metrics.get(), self.layout.borrow().clone())
        else {
            return;
        };
        let widths: Vec<i32> = self
            .entries
            .borrow()
            .iter()
            .map(|entry| width(&layout, entry, metrics))
            .collect();
        let rows = place(&widths, self.widget.allocated_width() - 2)
            .last()
            .map_or(1, |(row, _)| row + 1);
        let height = rows * metrics.height + 2;
        if self.widget.size_request().1 != height {
            self.widget.set_size_request(-1, height);
        }
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
            attention: RefCell::new(Vec::new()),
            bells: RefCell::new(Vec::new()),
            glow: Cell::new(0.0),
            blink: RefCell::new(None),
            drag: RefCell::new(None),
        });
        state.widget.add_events(
            gdk::EventMask::BUTTON_PRESS_MASK
                | gdk::EventMask::BUTTON_RELEASE_MASK
                | gdk::EventMask::BUTTON1_MOTION_MASK,
        );
        state.widget.connect_motion_notify_event(
            glib::clone!(@weak state => @default-return glib::Propagation::Proceed,
                move |_, event| {
                let (x, y) = event.position();
                state.drag_to(x, y);
                glib::Propagation::Proceed
            }),
        );
        state
            .widget
            .connect_size_allocate(glib::clone!(@weak state => move |_, _| state.fit()));
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
    pub fn connect_selected(&self, select: impl Fn(u32) + 'static, close: impl Fn(u32) + 'static) {
        let state = self.0.clone();
        self.0.widget.connect_button_press_event(glib::clone!(@weak state
            => @default-return glib::Propagation::Proceed, move |_, event| {
                let (x, y) = event.position();
                match event.button() {
                    1 => if let Some(id) = state.hit(x, y) {
                        *state.drag.borrow_mut() = Some(Drag { id, x, y, active: false, over: None });
                        select(id);
                        return glib::Propagation::Stop;
                    },
                    2 => if let Some(id) = state.hit(x, y) { close(id); return glib::Propagation::Stop; },
                    _ => {}
                }
                glib::Propagation::Proceed
            }));
    }
    /// Dropping a dragged tab on another asks to move it into that tab's slot: `moved(dragged, target)`.
    pub fn connect_moved(&self, moved: impl Fn(u32, u32) + 'static) {
        let state = self.0.clone();
        self.0.widget.connect_button_release_event(glib::clone!(@weak state
            => @default-return glib::Propagation::Proceed, move |_, event| {
                if event.button() != 1 { return glib::Propagation::Proceed; }
                let Some(drag) = state.drag.borrow_mut().take() else { return glib::Propagation::Proceed };
                if drag.active {
                    state.widget.queue_draw();
                    if let Some(target) = drag.over { moved(drag.id, target); }
                }
                glib::Propagation::Proceed
            }));
    }
    /// Fade a tab in and out until it receives input; a waiting agent is not answered by looking at it.
    /// Severity only rises until input answers the tab. Returns the new severity if it changed.
    pub fn alert(&self, id: u32, severity: u32) -> Option<u32> {
        let state = &self.0;
        let severity = severity.clamp(ATTENTION_DONE, ATTENTION_FAILED);
        {
            let mut attention = state.attention.borrow_mut();
            match attention.iter_mut().find(|(alerted, _)| *alerted == id) {
                Some((_, current)) if *current >= severity => return None,
                Some((_, current)) => *current = severity,
                None => attention.push((id, severity)),
            }
        }
        if state.blink.borrow().is_some() {
            state.widget.queue_draw();
            return Some(severity);
        }
        let start = std::time::Instant::now();
        state.glow.set(glow_at(0.0));
        state.widget.queue_draw();
        let weak = Rc::downgrade(state);
        *state.blink.borrow_mut() = Some(glib::timeout_add_local(
            std::time::Duration::from_millis(33),
            move || {
                let Some(state) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                if state.attention.borrow().is_empty() {
                    state.glow.set(0.0);
                    state.widget.queue_draw();
                    state.blink.borrow_mut().take();
                    return glib::ControlFlow::Break;
                }
                state.glow.set(glow_at(start.elapsed().as_secs_f64()));
                state.widget.queue_draw();
                glib::ControlFlow::Continue
            },
        ));
        Some(severity)
    }
    /// Count a bell into its tab's burst: one bell finished, two need the user, three failed.
    pub fn ring(&self, id: u32) -> u32 {
        let now = std::time::Instant::now();
        let mut bells = self.0.bells.borrow_mut();
        bells.retain(|(_, _, last)| now.duration_since(*last) < BELL_BURST);
        match bells.iter_mut().find(|(bell, _, _)| *bell == id) {
            Some((_, count, last)) => {
                *count = (*count + 1).min(ATTENTION_FAILED);
                *last = now;
                *count
            }
            None => {
                bells.push((id, 1, now));
                1
            }
        }
    }
    pub fn answer(&self, id: u32) {
        let mut attention = self.0.attention.borrow_mut();
        let count = attention.len();
        attention.retain(|(alert, _)| *alert != id);
        if attention.len() != count {
            self.0.widget.queue_draw();
        }
    }
    pub fn update(&self, entries: Vec<Tab>, active: u32, palette: Option<Rc<RefCell<Palette>>>) {
        self.0.active.set(active);
        // Closed tabs can never be answered.
        self.0
            .attention
            .borrow_mut()
            .retain(|(id, _)| entries.iter().any(|entry| entry.id == *id));
        *self.0.palette.borrow_mut() = palette;
        let empty = entries.is_empty();
        *self.0.entries.borrow_mut() = entries;
        if !empty {
            {
                let mut layout = self.0.layout.borrow_mut();
                let layout = layout.get_or_insert_with(|| self.0.widget.create_pango_layout(None));
                let metrics = self.0.fonts.borrow_mut().resolve(layout).metrics;
                self.0.metrics.set(Some(metrics));
            }
            self.0.fit();
        }
        self.0.widget.queue_draw();
    }
}

#[cfg(test)]
mod tests {
    use super::place;
    #[test]
    fn tabs_wrap_in_order_and_oversized_tab_sits_alone() {
        assert_eq!(place(&[], 100), vec![]);
        assert_eq!(place(&[30, 30, 30], 100), vec![(0, 2), (0, 32), (0, 62)]);
        assert_eq!(place(&[40, 40, 40], 100), vec![(0, 2), (0, 42), (1, 2)]);
        assert_eq!(place(&[500, 20, 20], 100), vec![(0, 2), (1, 2), (1, 22)]);
    }
}
