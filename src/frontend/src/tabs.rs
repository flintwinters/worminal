//! The one-cell Cairo strip shares terminal fonts/colors. Tabs wrap onto further rows in daemon order; drawing and hit testing share the row placement.
use crate::{
    fonts::Fonts,
    graphics_ffi,
    layout::Target,
    paint::{self, Metrics, Palette},
};
use gtk::{gdk, prelude::*};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use worminal_protocol::cell::{COLOR_DEFAULT_BACKGROUND as BG, COLOR_DEFAULT_FOREGROUND as FG};
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
    /// A task in the terminal is reporting progress.
    pub busy: bool,
    pub background_job: bool,
}
/// What the strip asks of the window. A tab is selected when its button is released, so a drag that
/// starts on it leaves the panes it is dragged over on screen.
pub struct Actions {
    pub select: Box<dyn Fn(u32)>,
    pub close: Box<dyn Fn(u32)>,
    /// Dropped on another tab: `moved(dragged, target)` asks to move it into that tab's slot.
    pub moved: Box<dyn Fn(u32, u32)>,
    /// The pane region a drop of `dragged` at this strip position would split, if any.
    pub locate: Box<dyn Fn(u32, f64, f64) -> Option<Target>>,
    /// The region to mark while a drag hovers it; `None` clears the mark.
    pub hover: Box<dyn Fn(Option<Target>)>,
    pub split: Box<dyn Fn(u32, Target)>,
    /// The dragged tab to draw under the pointer; `None` when the drag ends.
    pub ghost: Box<dyn Fn(Option<Ghost>)>,
}
/// A dragged tab's image, whose top-left sits at `x`, `y` in strip coordinates.
pub struct Ghost {
    pub image: cairo::ImageSurface,
    pub x: f64,
    pub y: f64,
}
/// A primary-button press on a tab. It becomes a reorder or a split once the pointer travels past `DRAG_SLOP`.
struct Drag {
    id: u32,
    x: f64,
    y: f64,
    active: bool,
    /// The tab whose slot the dragged tab takes if released now.
    over: Option<u32>,
    /// The pane region the dragged tab splits if released now; it wins over `over`.
    target: Option<Target>,
    /// The tab's image and the press point within it, made when the drag starts.
    ghost: Option<(cairo::ImageSurface, f64, f64)>,
}
const DRAG_SLOP: f64 = 6.0;
struct State {
    widget: gtk::DrawingArea,
    fonts: Rc<RefCell<Fonts>>,
    layout: RefCell<Option<pango::Layout>>,
    metrics: Cell<Option<Metrics>>,
    entries: RefCell<Vec<Tab>>,
    active: Cell<u32>,
    /// Whether this strip's pane has focus; only then does its selected tab get the solid highlight.
    focused: Cell<bool>,
    palette: RefCell<Option<Rc<RefCell<Palette>>>>,
    /// Tabs (with severity) that rang the bell and have not received input since; selecting a tab keeps it.
    attention: RefCell<Vec<(u32, u32)>>,
    /// Bells rung by each tab in its latest burst, with the time of the last one.
    bells: RefCell<Vec<(u32, u32, std::time::Instant)>>,
    drag: RefCell<Option<Drag>>,
    actions: RefCell<Option<Rc<Actions>>>,
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
/// ANSI magenta: tabs whose foreground is an SSH session.
const SSH: u32 = 5;
/// ANSI bright black: dark gray behind tabs whose task is busy.
const BUSY: u32 = 8;
fn width(layout: &pango::Layout, entry: &Tab, metrics: Metrics) -> i32 {
    layout.set_text(&entry.label);
    layout.pixel_size().0 + 2 * metrics.width
}
/// Each tab's `(x, top, width)` in strip coordinates; drawing, hit testing and drag images share it.
fn placed(
    layout: &pango::Layout,
    entries: &[Tab],
    metrics: Metrics,
    right: i32,
) -> Vec<(i32, i32, i32)> {
    let widths: Vec<i32> = entries
        .iter()
        .map(|entry| width(layout, entry, metrics))
        .collect();
    place(&widths, right)
        .into_iter()
        .zip(widths)
        .map(|((row, x), width)| (x, 2 + row * metrics.height, width))
        .collect()
}
/// One tab's fill, label and (when selected) underline, clipped at `right`.
#[expect(clippy::too_many_arguments, reason = "one tab's geometry and colors")]
fn paint_tab(
    cr: &cairo::Context,
    layout: &pango::Layout,
    metrics: Metrics,
    label: &str,
    (fill, text): (Option<gdk::RGBA>, gdk::RGBA),
    (x, top, width): (i32, i32, i32),
    right: i32,
    selected: bool,
) -> Result<(), cairo::Error> {
    layout.set_text(label);
    let baseline = top + (metrics.height - metrics.ink_height) / 2 + metrics.ascent;
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
    // The underline marks each pane's selected tab, focused or not.
    if selected {
        cr.rectangle(
            f64::from(x + metrics.width),
            f64::from((baseline + 2).min(top - 2 + metrics.height)),
            f64::from((width - 2 * metrics.width).max(0)),
            1.0,
        );
        cr.fill()?;
    }
    Ok(())
}
pub(super) fn mix(from: &gdk::RGBA, to: &gdk::RGBA, amount: f64) -> gdk::RGBA {
    let blend = |a: f64, b: f64| a + (b - a) * amount;
    gdk::RGBA::new(
        blend(from.red(), to.red()),
        blend(from.green(), to.green()),
        blend(from.blue(), to.blue()),
        1.0,
    )
}
/// Background fill (none for resting tabs) and text color; attention stays solid until input.
fn colors(
    palette: &Palette,
    entry: &Tab,
    selected: bool,
    severity: u32,
) -> (Option<gdk::RGBA>, gdk::RGBA) {
    if severity > 0 {
        return (
            Some(palette.color(severity_color(severity))),
            palette.color(BG),
        );
    }
    let fill = if entry.background_job {
        Some(palette.color(6))
    } else if selected {
        Some(palette.color(if entry.ssh { SSH } else { FG }))
    } else {
        entry.busy.then(|| palette.color(BUSY))
    };
    let text = palette.color(if selected || entry.background_job {
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
    (fill, text)
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
        let placed = placed(layout, &entries, metrics, right);
        for (i, (entry, &(x, top, width))) in entries.iter().zip(&placed).enumerate() {
            cr.save()?;
            cr.rectangle(
                2.0,
                f64::from(top),
                f64::from((right - 2).max(0)),
                f64::from(metrics.height),
            );
            cr.clip();
            let selected = entry.id == self.active.get();
            let colors = colors(
                &palette,
                entry,
                selected && self.focused.get(),
                self.severity(entry.id),
            );
            paint_tab(
                cr,
                layout,
                metrics,
                &entry.label,
                colors,
                (x, top, width),
                right,
                selected,
            )?;
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
        entries
            .iter()
            .zip(placed(layout, &entries, metrics, right))
            .find(|(_, (left, top, width))| {
                y >= f64::from(*top)
                    && y < f64::from(top + metrics.height)
                    && x >= f64::from(*left)
                    && x < f64::from((left + width).min(right))
            })
            .map(|(entry, _)| entry.id)
    }
    fn severity(&self, id: u32) -> u32 {
        self.attention
            .borrow()
            .iter()
            .find(|(alert, _)| *alert == id)
            .map_or(0, |(_, severity)| *severity)
    }
    /// The tab as a focused selection, with the strip point `(x, y)` relative to its top-left.
    fn image(&self, id: u32, x: f64, y: f64) -> Option<(cairo::ImageSurface, f64, f64)> {
        let metrics = self.metrics.get()?;
        let layout = self.layout.borrow();
        let layout = layout.as_ref()?;
        let palette = self.palette.borrow();
        let palette = palette.as_ref()?.borrow();
        let entries = self.entries.borrow();
        let right = self.widget.allocated_width() - 2;
        let (entry, &(left, top, width)) = entries
            .iter()
            .zip(&placed(layout, &entries, metrics, right))
            .find(|(entry, _)| entry.id == id)?;
        let width = width.min(right - left).max(1);
        let scale = self.widget.scale_factor();
        let image = cairo::ImageSurface::create(
            cairo::Format::ARgb32,
            width * scale,
            metrics.height * scale,
        )
        .ok()?;
        image.set_device_scale(f64::from(scale), f64::from(scale));
        let cr = cairo::Context::new(&image).ok()?;
        cr.translate(f64::from(-left), f64::from(-top));
        let colors = colors(&palette, entry, true, self.severity(id));
        paint_tab(
            &cr,
            layout,
            metrics,
            &entry.label,
            colors,
            (left, top, width),
            left + width,
            true,
        )
        .ok()?;
        drop(cr);
        Some((image, x - f64::from(left), y - f64::from(top)))
    }
    fn drag_to(&self, x: f64, y: f64) {
        let over = self.hit(x, y);
        let Some((id, started)) = self.drag.borrow().as_ref().map(|drag| {
            (
                drag.id,
                drag.active || (x - drag.x).hypot(y - drag.y) >= DRAG_SLOP,
            )
        }) else {
            return;
        };
        if !started {
            return;
        }
        // Window callbacks run with no strip state borrowed.
        let actions = self.actions.borrow().clone();
        // The image keeps the point where the tab was grabbed under the pointer.
        let press = self
            .drag
            .borrow()
            .as_ref()
            .filter(|drag| drag.ghost.is_none())
            .map(|drag| (drag.x, drag.y));
        let image = press.and_then(|(px, py)| self.image(id, px, py));
        let target = actions
            .as_ref()
            .and_then(|actions| (actions.locate)(id, x, y));
        let changed = {
            let mut drag = self.drag.borrow_mut();
            let Some(drag) = drag.as_mut() else { return };
            // Keep the last tab while the pointer crosses gaps; a pane region replaces it.
            let over = if target.is_some() {
                None
            } else {
                over.filter(|over| *over != drag.id).or(drag.over)
            };
            let changed = !drag.active || over != drag.over || target != drag.target;
            let moved = target != drag.target;
            drag.active = true;
            drag.over = over;
            drag.target = target;
            drag.ghost = drag.ghost.take().or(image);
            let ghost = drag.ghost.as_ref().map(|(image, dx, dy)| Ghost {
                image: image.clone(),
                x: x - dx,
                y: y - dy,
            });
            (changed.then_some(moved), ghost)
        };
        let (changed, ghost) = changed;
        if let (Some(actions), Some(ghost)) = (actions.as_ref(), ghost) {
            (actions.ghost)(Some(ghost));
        }
        if let Some(moved) = changed {
            self.widget.queue_draw();
            if let Some(actions) = actions.filter(|_| moved) {
                (actions.hover)(target);
            }
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
            focused: Cell::new(false),
            palette: RefCell::new(None),
            attention: RefCell::new(Vec::new()),
            bells: RefCell::new(Vec::new()),
            drag: RefCell::new(None),
            actions: RefCell::new(None),
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
        // GTK drops a resize queued while the pane is being allocated, which left a narrowed strip on its old
        // row count; fitting once the allocation has finished gets the new height laid out.
        state
            .widget
            .connect_size_allocate(glib::clone!(@weak state => move |_, _| {
                glib::idle_add_local_once(glib::clone!(@weak state => move || state.fit()));
            }));
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
    pub fn connect(&self, actions: Actions) {
        let state = self.0.clone();
        *state.actions.borrow_mut() = Some(Rc::new(actions));
        self.0.widget.connect_button_press_event(glib::clone!(@weak state
            => @default-return glib::Propagation::Proceed, move |_, event| {
                let (x, y) = event.position();
                let Some(id) = state.hit(x, y) else { return glib::Propagation::Proceed };
                match event.button() {
                    1 => *state.drag.borrow_mut() = Some(Drag { id, x, y, active: false, over: None, target: None, ghost: None }),
                    2 => {
                        let actions = state.actions.borrow().clone();
                        if let Some(actions) = actions { (actions.close)(id); }
                    }
                    _ => return glib::Propagation::Proceed,
                }
                glib::Propagation::Stop
            }));
        self.0.widget.connect_button_release_event(glib::clone!(@weak state
            => @default-return glib::Propagation::Proceed, move |_, event| {
                if event.button() != 1 { return glib::Propagation::Proceed; }
                let Some(drag) = state.drag.borrow_mut().take() else { return glib::Propagation::Proceed };
                let Some(actions) = state.actions.borrow().clone() else { return glib::Propagation::Proceed };
                if drag.active {
                    state.widget.queue_draw();
                    (actions.hover)(None);
                    (actions.ghost)(None);
                }
                match (drag.target, drag.over) {
                    (Some(target), _) => (actions.split)(drag.id, target),
                    (None, over) => {
                        if let Some(over) = over { (actions.moved)(drag.id, over); }
                        (actions.select)(drag.id);
                    }
                }
                glib::Propagation::Proceed
            }));
    }
    /// Highlight a tab until it receives input; a waiting agent is not answered by looking at it.
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
        state.widget.queue_draw();
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
    pub fn focus(&self, focused: bool) {
        if self.0.focused.replace(focused) != focused {
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
    use super::{colors, place, severity_color, Tab, ATTENTION_HELP, BUSY, FG};
    use crate::paint::Palette;
    fn tab(busy: bool) -> Tab {
        Tab {
            id: 1,
            label: String::new(),
            browser: false,
            remote: false,
            ssh: false,
            busy,
            background_job: false,
        }
    }
    #[test]
    fn a_busy_tab_is_gray_unless_selected_or_alerting() {
        let palette = Palette::default();
        assert_eq!(colors(&palette, &tab(false), false, 0).0, None);
        assert_eq!(
            colors(&palette, &tab(true), false, 0).0,
            Some(palette.color(BUSY))
        );
        assert_eq!(
            colors(&palette, &tab(true), true, 0).0,
            Some(palette.color(FG))
        );
        assert_eq!(
            colors(&palette, &tab(true), false, ATTENTION_HELP).0,
            Some(palette.color(severity_color(ATTENTION_HELP)))
        );
    }
    #[test]
    fn background_jobs_are_cyan_including_selected_and_busy_tabs() {
        let palette = Palette::default();
        let mut entry = tab(true);
        entry.background_job = true;
        for selected in [false, true] {
            let (fill, text) = colors(&palette, &entry, selected, 0);
            assert_eq!(fill, Some(palette.color(6)));
            assert_eq!(text, palette.color(super::BG));
            assert_eq!(
                colors(&palette, &entry, selected, ATTENTION_HELP).0,
                Some(palette.color(severity_color(ATTENTION_HELP)))
            );
        }
    }
    #[test]
    fn tabs_wrap_in_order_and_oversized_tab_sits_alone() {
        assert_eq!(place(&[], 100), vec![]);
        assert_eq!(place(&[30, 30, 30], 100), vec![(0, 2), (0, 32), (0, 62)]);
        assert_eq!(place(&[40, 40, 40], 100), vec![(0, 2), (0, 42), (1, 2)]);
        assert_eq!(place(&[500, 20, 20], 100), vec![(0, 2), (1, 2), (1, 22)]);
    }
}
