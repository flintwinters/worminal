use super::App;
use gtk::{
    gdk::{self, keys::constants as key, ModifierType as Mod},
    prelude::*,
};
use std::rc::Rc;
use worminal_protocol::{WIRE_CLOSE, WIRE_FEATURE_SELECT_ALL};

impl App {
    #[expect(clippy::too_many_lines, reason = "one arm per keyboard shortcut")]
    pub(super) fn shortcut(self: &Rc<Self>, event: &gdk::EventKey) -> bool {
        // Windows map before their initial catalog arrives. Preserve early keystrokes until a widget exists.
        if self.active.get() == 0
            && (self.initial_claim.get()
                || self.pending_tabs.get() != 0
                || self.requested.get() != 0)
        {
            let mut early = self.early_keys.borrow_mut();
            if early.len() < 256 {
                early.push_back(event.clone());
            }
            return true;
        }
        let value = event.keyval().to_lower();
        if let Some(browser) = self.browser(self.active.get()) {
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
                if self.service_features.get() & WIRE_FEATURE_SELECT_ALL == 0 {
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
                self.send(self.active.get(), WIRE_CLOSE, Vec::new());
            }
            return true;
        }
        if (state == Mod::CONTROL_MASK || state == control_shift)
            && [key::Tab, key::ISO_Left_Tab].contains(&value)
        {
            let count = self.notebook.n_pages();
            if count != 0 {
                let current = self.notebook.current_page().unwrap_or(0);
                self.select_index(if state == control_shift {
                    (current + count - 1) % count
                } else {
                    (current + 1) % count
                });
            }
            return true;
        }
        let mut zoom = self.fonts.borrow().zoom();
        if state == control_shift {
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
                    if let Err(error) = Self::open(config) {
                        self.message(Some(&error.to_string()));
                    }
                    return true;
                }
                key::w => {
                    self.send(self.active.get(), WIRE_CLOSE, Vec::new());
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
                let count = self.notebook.n_pages();
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
        self.fonts.borrow_mut().set_zoom(zoom.clamp(-10.0, 40.0));
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
        true
    }
}
