//! The side panel's window glue: opening and closing it, the lists it shows and what its actions do.
use super::{bookmarks::store, App};
use crate::panel::{BookmarksTab, Callbacks, PanelAction, PanelTab};
use gtk::prelude::*;
use std::rc::Rc;

impl App {
    /// Ctrl+Shift+B (or P): open the panel; if it is open but the keyboard is elsewhere, move the keyboard to it;
    /// if it already has the keyboard, close it.
    pub(super) fn toggle_panel(self: &Rc<Self>) {
        if !self.panel.is_open() {
            self.open_panel();
        } else if self.panel.has_focus() {
            self.close_panel();
        } else {
            self.panel.focus();
        }
    }
    fn open_panel(self: &Rc<Self>) {
        // Another window may have changed the list since this one last looked.
        self.reload_bookmarks();
        let palette = {
            let pages = self.pages.borrow();
            pages
                .iter()
                .find(|page| page.id == self.active.get())
                .or_else(|| pages.first())
                .map(|page| page.palette.clone())
        };
        let Some(palette) = palette else { return };
        self.panel.open(
            self.fonts.clone(),
            palette,
            self.panel_tabs(),
            Callbacks {
                act: Box::new(
                    glib::clone!(@weak self as app => move |action| app.panel_action(action)),
                ),
                close: Box::new(glib::clone!(@weak self as app => move || app.close_panel())),
            },
        );
    }
    /// Free the panel and give the keyboard back to the tab being viewed.
    pub(super) fn close_panel(self: &Rc<Self>) {
        self.panel.close();
        match self.terminal(self.active.get()) {
            Some(terminal) => terminal.widget().grab_focus(),
            None if self.active.get() != 0 => self.claim_browser(self.active.get()),
            None => {}
        }
    }
    /// The panel's tabs, in order; Bookmarks first. New lists join here.
    fn panel_tabs(&self) -> Vec<Box<dyn PanelTab>> {
        vec![Box::new(BookmarksTab::new(
            self.bookmarks.borrow().clone(),
            std::env::var("HOME").ok(),
        ))]
    }
    pub(super) fn refresh_panel(&self) {
        if self.panel.is_open() {
            self.panel.set_tabs(self.panel_tabs());
        }
    }
    pub(super) fn panel_action(self: &Rc<Self>, action: PanelAction) {
        match action {
            PanelAction::RemoveBookmark(target) => match store().remove(&target) {
                Ok(_) => self.reload_bookmarks(),
                Err(error) => self.message(Some(&error.to_string())),
            },
            // The new tab takes the keyboard when it arrives, so the panel only has to get out of the way.
            PanelAction::OpenTerminal { directory, master } => {
                self.close_panel();
                self.open_bookmarked_terminal(directory, master);
            }
            PanelAction::AddBookmark(bookmark) => match store().add(bookmark) {
                Ok(_) => self.reload_bookmarks(),
                Err(error) => self.message(Some(&error.to_string())),
            },
            PanelAction::OpenHost(endpoint) => {
                self.close_panel();
                self.open_bookmarked_host(endpoint);
            }
            PanelAction::OpenUrl(url) => {
                self.close_panel();
                self.open_browser(&url);
            }
        }
    }
}
