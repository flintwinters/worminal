use super::App;
use gtk::{
    gdk::{self, keys::constants as key, ModifierType as Mod},
    prelude::*,
};
use std::rc::Rc;
use worminal_protocol::WIRE_CLOSE;

impl App {
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
        let state = event.state() & (Mod::CONTROL_MASK | Mod::SHIFT_MASK | Mod::MOD1_MASK);
        let control_shift = Mod::CONTROL_MASK | Mod::SHIFT_MASK;
        if state == Mod::CONTROL_MASK && [key::t, key::w].contains(&value) {
            if value == key::t {
                self.new_tab();
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
                self.notebook
                    .set_current_page(Some(if state == control_shift {
                        (current + count - 1) % count
                    } else {
                        (current + 1) % count
                    }));
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
                    config.title = "Worminal Rust".into();
                    config.output.clear();
                    config.line.clear();
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
                    self.notebook.set_current_page(Some(index.min(count - 1)));
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
        true
    }
}
