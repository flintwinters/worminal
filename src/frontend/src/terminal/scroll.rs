//! Wheel dispatch and application scrolling policy.
use super::*;
use worminal_protocol::input::MAX_WHEEL_NOTCHES;

impl Inner {
    pub(super) fn connect_wheel(self: &Rc<Self>) {
        self.widget.connect_scroll_event(
            glib::clone!(@weak self as inner => @default-return glib::Propagation::Proceed,
            move |_, event| { inner.wheel(event); glib::Propagation::Stop }),
        );
    }

    pub(super) fn wheel(self: &Rc<Self>, event: &gdk::EventScroll) {
        let smooth = event.direction() == gdk::ScrollDirection::Smooth;
        if smooth && event.delta().1 == 0.0 {
            return;
        }
        if event.state().contains(gdk::ModifierType::CONTROL_MASK) {
            let steps = match event.direction() {
                gdk::ScrollDirection::Up => -1.0,
                gdk::ScrollDirection::Down => 1.0,
                gdk::ScrollDirection::Smooth => {
                    let mut state = self.state.borrow_mut();
                    state.tab_scroll += event.delta().1;
                    let steps = state.tab_scroll.trunc();
                    state.tab_scroll -= steps;
                    steps
                }
                _ => return,
            };
            self.stop_autoscroll();
            for _ in 0..(steps.abs() as u32).min(32) {
                (self.sink)(Action::CycleTab(steps < 0.0));
            }
            return;
        }
        self.state.borrow_mut().tab_scroll = 0.0;
        self.prepare();
        let up = event.direction() == gdk::ScrollDirection::Up || smooth && event.delta().1 < 0.0;
        let (px, py) = event.position();
        let (x, y) = self.pointer(px, py);
        let lines = worminal_config::get().scroll_multiplier as u32;
        self.scroll(up, 1, lines, (x, y), event.state());
    }
    /// Scrolls as the wheel does: applications owning the mouse get `notches` wheel reports, the
    /// alternate screen gets `notches` arrow keys, and otherwise the daemon scrolls `lines` of history.
    pub(super) fn scroll(
        self: &Rc<Self>,
        up: bool,
        notches: u32,
        lines: u32,
        (x, y): (i32, i32),
        mods: gdk::ModifierType,
    ) {
        self.deliver(CommandKind::Wheel {
            up,
            notches: notches.min(MAX_WHEEL_NOTCHES),
            lines,
            column: x.max(0) as u32,
            row: y.max(0) as u32,
            modifiers: mods.bits() & MOD_KEYS,
        });
    }
}
