use super::App;
use crate::layout::Direction;
use gtk::{
    gdk::{self, keys::constants as key, ModifierType as Mod},
    prelude::*,
};
use std::rc::Rc;
use worminal_protocol::worp::commands::CommandKind;
use worminal_protocol::WIRE_FEATURE_SELECT_ALL;

impl App {
    #[expect(clippy::too_many_lines, reason = "one arm per keyboard shortcut")]
    pub(super) fn shortcut(self: &Rc<Self>, event: &gdk::EventKey) -> bool {
        if self.replacing.get() {
            return true;
        }
        // Windows map before their initial catalog arrives. Preserve early keystrokes until a widget exists.
        if self.active.get() == 0
            && (self.primary.initial_claim.get()
                || self.primary.pending_tabs.get() != 0
                || self.primary.requested.get() != 0)
        {
            let mut early = self.early_keys.borrow_mut();
            if early.len() < 256 {
                early.push_back(event.clone());
            }
            return true;
        }
        let value = event.keyval().to_lower();
        // The panel's own keys (Escape, arrows, Enter) must reach it, not the page behind it.
        if let Some(browser) = self
            .browser(self.active.get())
            .filter(|_| !self.panel.has_focus())
        {
            if browser.shortcut(event) {
                return true;
            }
        }
        let state = event.state() & (Mod::CONTROL_MASK | Mod::SHIFT_MASK | Mod::MOD1_MASK);
        let control_shift = Mod::CONTROL_MASK | Mod::SHIFT_MASK;
        let browser = self
            .pages
            .borrow()
            .iter()
            .any(|page| page.id == self.active.get() && page.browser);
        if state == control_shift && value == key::a {
            if let Some(terminal) = self.terminal(self.active.get()) {
                if self.features(self.active.get()) & WIRE_FEATURE_SELECT_ALL == 0 {
                    self.message(Some("Select all requires an updated workspace service."));
                } else {
                    terminal.select_all();
                }
                return true;
            }
        }
        if state == Mod::CONTROL_MASK
            && ([key::t, key::w].contains(&value) || (browser && [key::d, key::q].contains(&value)))
        {
            if value == key::t {
                if self
                    .pages
                    .borrow()
                    .iter()
                    .any(|page| page.id == self.active.get() && page.browser)
                {
                    self.open_browser(worminal_workspace::WORKSPACE_BROWSER_HOME);
                } else {
                    self.new_tab();
                }
            } else {
                let closing = self.active.get();
                if value == key::q {
                    // Quitting a browser returns to a shell, as exiting a program does. The shell that ran
                    // `--new-browser-tab` ended when the browser replaced it, so a fresh one takes its slot:
                    // the daemon places the new tab after the browser, which then closes beside it.
                    self.new_tab();
                }
                self.send(closing, CommandKind::Close { receipt: false });
            }
            return true;
        }
        // Alt+Tab cycles like Ctrl+Tab but only through tabs that have a colored background (alerts, busy
        // tasks, background jobs).
        if [
            Mod::CONTROL_MASK,
            control_shift,
            Mod::MOD1_MASK,
            Mod::MOD1_MASK | Mod::SHIFT_MASK,
        ]
        .contains(&state)
            && [key::Tab, key::ISO_Left_Tab].contains(&value)
        {
            self.cycle_tab(
                state.contains(Mod::SHIFT_MASK),
                state.contains(Mod::MOD1_MASK),
            );
            return true;
        }
        // Ctrl+Shift+arrow focuses the pane that way; Alt+Shift+arrow moves the tab there like a drag would.
        // Each leaves the keystroke to the application when it has nothing to do (no split, nothing to move).
        // Ctrl+Shift+Alt+arrow is not used: GNOME owns it and some keyboards cannot send it.
        let move_tab = state == Mod::MOD1_MASK | Mod::SHIFT_MASK;
        if state == control_shift || move_tab {
            let direction = match value {
                key::Left => Some(Direction::Left),
                key::Right => Some(Direction::Right),
                key::Up => Some(Direction::Up),
                key::Down => Some(Direction::Down),
                _ => None,
            };
            if let Some(direction) = direction {
                return if move_tab {
                    self.move_toward(direction)
                } else {
                    self.focus_toward(direction)
                };
            }
        }
        let mut zoom = self.fonts.borrow().zoom();
        if state == control_shift {
            // A paste while the panel is asking for a host goes to that question, not to the terminal beside it.
            if value == key::v && self.panel.paste() {
                return true;
            }
            match value {
                key::t => {
                    self.new_tab();
                    return true;
                }
                key::n => {
                    let mut config = self.config.clone();
                    config.command.clear();
                    config.attach = false;
                    config.title = if config.candidate {
                        "Worminal Rust"
                    } else {
                        "Worminal"
                    }
                    .into();
                    config.output.clear();
                    config.line.clear();
                    config.browser_url = None;
                    config.zoom = self.fonts.borrow().zoom();
                    if let Err(error) = Self::launch(config) {
                        self.message(Some(&error.to_string()));
                    }
                    return true;
                }
                key::w => {
                    self.send(self.active.get(), CommandKind::Close { receipt: false });
                    return true;
                }
                key::x => {
                    self.unsplit();
                    return true;
                }
                // Close the window and forget its layout, so the next start is fresh.
                key::q => {
                    self.close_without_layout();
                    return true;
                }
                // B for bookmarks; P also works for keyboards that can send it (a membrane keyboard may not).
                key::b | key::p => {
                    self.toggle_panel();
                    return true;
                }
                key::d => {
                    self.toggle_bookmark(self.active.get());
                    return true;
                }
                key::c | key::v | key::y => {
                    if let Some(terminal) = self.terminal(self.active.get()) {
                        if value == key::c {
                            terminal.copy(gdk::SELECTION_CLIPBOARD);
                        } else {
                            terminal.paste(if value == key::y {
                                gdk::SELECTION_PRIMARY
                            } else {
                                gdk::SELECTION_CLIPBOARD
                            });
                        }
                    } else {
                        return false;
                    }
                    return true;
                }
                key::Home => zoom = 0.0,
                key::Page_Up | key::plus => zoom += 1.0,
                key::Page_Down => zoom -= 1.0,
                _ => return false,
            }
        } else if state == Mod::CONTROL_MASK && [key::equal, key::minus].contains(&value) {
            zoom += if value == key::equal { 1.0 } else { -1.0 };
        } else if state == Mod::CONTROL_MASK || state == Mod::MOD1_MASK {
            if let Some(c @ '0'..='9') = value.to_unicode() {
                let count = self.notebook().n_pages();
                if count != 0 {
                    let index = match c {
                        '9' => count - 1,
                        '0' => 9,
                        _ => u32::from(c) - u32::from('1'),
                    };
                    self.select_index(index.min(count - 1));
                }
                return true;
            }
            return false;
        } else {
            return false;
        }
        self.set_zoom(zoom);
        true
    }

    pub(super) fn set_zoom(self: &Rc<Self>, zoom: f64) {
        // Every page must receive the same bounded zoom, including browser views.
        let zoom = zoom.clamp(-10.0, 40.0);
        self.fonts.borrow_mut().set_zoom(zoom);
        let terminals: Vec<_> = self
            .pages
            .borrow()
            .iter()
            .filter_map(|page| page.terminal.clone())
            .collect();
        for terminal in terminals {
            terminal.zoom();
        }
        for page in self.pages.borrow().iter() {
            if let Some(browser) = page.view.borrow().as_ref() {
                browser.zoom(zoom);
            }
        }
        self.refresh_tabs();
    }

    /// Select the next (or previous) tab; `tinted_only` skips tabs without a background fill in their strip.
    pub(super) fn cycle_tab(self: &Rc<Self>, previous: bool, tinted_only: bool) {
        let count = self.notebook().n_pages();
        let current = self.notebook().current_page().unwrap_or(0);
        let candidates = (1..count).map(|step| {
            if previous {
                (current + count - step) % count
            } else {
                (current + step) % count
            }
        });
        let target = candidates.into_iter().find(|index| {
            !tinted_only || {
                let widget = self.notebook().nth_page(Some(*index));
                let id = self
                    .pages
                    .borrow()
                    .iter()
                    .find(|page| Some(&page.widget) == widget.as_ref())
                    .map(|page| page.id);
                id.is_some_and(|id| self.panes().iter().any(|pane| pane.strip.tinted(id)))
            }
        });
        if let Some(index) = target {
            self.select_index(index);
        }
    }
}
