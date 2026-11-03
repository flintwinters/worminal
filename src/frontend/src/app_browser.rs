//! One live browser view per local workspace tab; moving it preserves the DOM.
use super::{App, Record, WINDOWS};
use crate::browser::Browser;
use gtk::prelude::*;
use std::{ffi::OsString, rc::Rc};
use worminal_browser_api::*;
use worminal_protocol::*;

impl App {
    pub(super) fn release_browsers(self: &Rc<Self>) {
        let ids: Vec<_> = self
            .pages
            .borrow()
            .iter()
            .filter(|page| page.view.borrow().is_some())
            .map(|page| page.id)
            .collect();
        for id in ids {
            let peer = WINDOWS.with(|windows| {
                windows
                    .borrow()
                    .iter()
                    .find(|app| {
                        !Rc::ptr_eq(app, self)
                            && !app.closing.get()
                            && app.config.master == self.config.master
                            && app
                                .pages
                                .borrow()
                                .iter()
                                .any(|page| page.id == id && page.browser)
                    })
                    .cloned()
            });
            if let Some(peer) = peer {
                peer.claim_browser(id);
            }
        }
    }
    pub(super) fn browser(&self, id: u32) -> Option<Browser> {
        self.pages
            .borrow()
            .iter()
            .find(|page| page.id == id)?
            .view
            .borrow()
            .clone()
    }
    pub(super) fn open_browser(self: &Rc<Self>, url: &str) {
        let mut body = Buffer::default();
        match body.put_string(url.as_bytes()) {
            Ok(()) => self.send(0, WIRE_BROWSER, body.into_bytes()),
            Err(error) => self.message(Some(&error.to_string())),
        }
    }
    pub(super) fn claim_browser(self: &Rc<Self>, id: u32) {
        let Some(page) = self
            .pages
            .borrow()
            .iter()
            .find(|page| page.id == id && page.browser)
            .cloned()
        else {
            return;
        };
        let container = page
            .widget
            .clone()
            .downcast::<gtk::Box>()
            .expect("browser container");
        if page.view.borrow().is_none() {
            let master = self.config.master.clone();
            let peer = WINDOWS.with(|windows| {
                windows
                    .borrow()
                    .iter()
                    .filter(|app| !Rc::ptr_eq(app, self) && app.config.master == master)
                    .find_map(|app| {
                        app.pages
                            .borrow()
                            .iter()
                            .find(|page| page.id == id && page.browser)
                            .filter(|page| page.view.borrow().is_some())
                            .cloned()
                    })
            });
            let is_new = peer.is_none();
            let view = if let Some(peer) = peer {
                let view = peer.view.borrow_mut().take().unwrap();
                if let Some(parent) = view
                    .widget()
                    .parent()
                    .and_then(|parent| parent.downcast::<gtk::Container>().ok())
                {
                    parent.remove(view.widget());
                }
                for child in peer
                    .widget
                    .clone()
                    .downcast::<gtk::Box>()
                    .unwrap()
                    .children()
                {
                    child.show();
                }
                Ok(view)
            } else {
                Browser::new(&page.url, move |kind, value| {
                    event(&master, id, kind, value)
                })
            };
            match view {
                Ok(view) => {
                    for child in container.children() {
                        child.hide();
                    }
                    container.pack_start(view.widget(), true, true, 0);
                    *page.view.borrow_mut() = Some(view.clone());
                    view.widget().show_all();
                    view.zoom(self.fonts.borrow().zoom());
                    view.background(page.palette.borrow().color(259));
                    // New views start blank; moved views retain their current document and page state.
                    if is_new {
                        view.navigate(&page.url);
                    }
                }
                Err(error) => {
                    self.message(Some(&error.to_string()));
                    return;
                }
            }
        }
        if let Some(view) = page.view.borrow().clone() {
            view.focus();
        }
        self.send(
            id,
            WIRE_FOCUS,
            [self.config.columns, self.config.rows]
                .into_iter()
                .flat_map(u32::to_be_bytes)
                .collect(),
        );
    }
    pub(super) fn browser_metadata(self: &Rc<Self>, record: &Record) {
        let changed = {
            let mut pages = self.pages.borrow_mut();
            let Some(page) = pages.iter_mut().find(|page| page.id == record.id) else {
                return;
            };
            let changed = page.url != record.url;
            page.url = record.url.clone();
            changed
        };
        if changed {
            if let Some(view) = self.browser(record.id) {
                view.navigate(&record.url);
            }
        }
    }
}
fn event(master: &Option<OsString>, id: u32, kind: u32, value: &str) {
    let owners: Vec<_> = WINDOWS.with(|windows| {
        windows
            .borrow()
            .iter()
            .filter(|app| !app.closing.get() && &app.config.master == master)
            .cloned()
            .collect()
    });
    let owner = owners.iter().find(|app| app.browser(id).is_some());
    if kind == WORMINAL_BROWSER_TITLE {
        for app in &owners {
            if let Some(page) = app
                .pages
                .borrow_mut()
                .iter_mut()
                .find(|page| page.id == id && page.browser)
            {
                page.title = value.to_string();

                if app.active.get() == id {
                    app.window.set_title(value);
                }
            }
            app.refresh_tabs();
        }
    } else if let Some(app) = owner {
        if kind == WORMINAL_BROWSER_OPEN {
            app.open_browser(value);
        } else if kind == WORMINAL_BROWSER_CLOSE {
            app.send(id, WIRE_CLOSE, Vec::new());
        } else if kind == WORMINAL_BROWSER_NAVIGATE && !value.is_empty() {
            let mut changed = false;
            for peer in &owners {
                if let Some(page) = peer
                    .pages
                    .borrow_mut()
                    .iter_mut()
                    .find(|page| page.id == id && page.browser)
                {
                    changed |= page.url != value;
                    page.url = value.to_string();
                }
            }
            if changed {
                let mut body = Buffer::default();
                if body.put_string(value.as_bytes()).is_ok() {
                    app.send(id, WIRE_BROWSER_NAVIGATE, body.into_bytes());
                }
            }
        }
    }
}
