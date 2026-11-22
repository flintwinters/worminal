//! One live browser view per local workspace tab; moving it preserves the DOM.
use super::{App, Record, WINDOWS};
use crate::browser::Browser;
use gtk::prelude::*;
use std::{ffi::OsString, rc::Rc};
use worminal_browser_api::*;
use worminal_protocol::worp::commands::CommandKind;

/// Browser tabs are the only reason to keep the profile's network process.
pub(super) fn release_unused_profile() {
    let browsing = WINDOWS.with(|windows| {
        windows
            .borrow()
            .iter()
            .any(|app| app.pages.borrow().iter().any(|page| page.browser))
    });
    if !browsing {
        crate::browser::release_profile();
    }
}
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
        self.send(
            0,
            CommandKind::Browser {
                url: url.as_bytes().to_vec(),
                subscribe: true,
            },
        );
    }
    /// Move the tab's view into this window, or create it, and focus it.
    pub(super) fn claim_browser(self: &Rc<Self>, id: u32) {
        let Some(view) = self.host_browser(id) else {
            return;
        };
        view.focus();
        // `Focus` is exclusive: it would hide the terminals a split shows beside this browser and stop their frames.
        // The visible terminals are presented instead; with none visible the browser's tab is focused as before.
        if self
            .visible()
            .iter()
            .any(|shown| self.terminal(*shown).is_some())
        {
            self.present(true);
        } else {
            self.send(
                id,
                CommandKind::Focus {
                    columns: self.config.columns,
                    rows: self.config.rows,
                },
            );
        }
    }
    /// A split pane's selected browser shows its page without being the active tab. Views another window hosts
    /// stay there: only claiming moves a live DOM between windows.
    pub(super) fn show_visible_browsers(self: &Rc<Self>) {
        if self.replacing.get() {
            return;
        }
        for id in self.visible() {
            let hosted = WINDOWS.with(|windows| {
                windows.borrow().iter().any(|app| {
                    app.pages
                        .borrow()
                        .iter()
                        .any(|page| page.id == id && page.view.borrow().is_some())
                })
            });
            let browser = self
                .pages
                .borrow()
                .iter()
                .any(|page| page.id == id && page.browser);
            if browser && !hosted {
                self.host_browser(id);
            }
        }
    }
    fn host_browser(self: &Rc<Self>, id: u32) -> Option<Browser> {
        let page = self
            .pages
            .borrow()
            .iter()
            .find(|page| page.id == id && page.browser)
            .cloned()?;
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
                    host(&container, view.widget());
                    *page.view.borrow_mut() = Some(view.clone());
                    view.widget().show_all();
                    view.zoom(self.fonts.borrow().zoom());
                    view.background(page.palette.borrow().color(259));
                    // New views start blank; moved views retain their current document and page state.
                    if is_new && !self.defer_navigation.get() {
                        view.navigate(&page.url);
                    }
                }
                Err(error) => {
                    self.message(Some(&error.to_string()));
                    return None;
                }
            }
        }
        let view = page.view.borrow().clone();
        view
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
        if changed && !self.defer_navigation.get() {
            if let Some(view) = self.browser(record.id) {
                view.navigate(&record.url);
            }
        }
    }
}
/// A browser page's container: a button that claims the live view for this window until it does.
pub(super) fn placeholder(claim: impl Fn() + 'static) -> gtk::Box {
    let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let button = gtk::Button::with_label("Show browser here");
    button.connect_clicked(move |_| claim());
    // Split changes `show_all` the pane tree; an ancestor must not reveal the button beside a hosted view,
    // where it takes an equal share of the page. Only `host` and a peer's release change its visibility.
    button.set_no_show_all(true);
    button.show();
    container.pack_start(&button, true, true, 0);
    container
}
/// Replace the placeholder with the live view.
fn host(container: &gtk::Box, view: &gtk::Widget) {
    for child in container.children() {
        child.hide();
    }
    container.pack_start(view, true, true, 0);
}
fn event(master: &Option<OsString>, id: u32, kind: u32, value: &str) {
    let owners: Vec<_> = WINDOWS.with(|windows| {
        windows
            .borrow()
            .iter()
            .filter(|app| {
                !app.closing.get() && !app.replacing.get() && &app.config.master == master
            })
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
            app.send(id, CommandKind::Close { receipt: false });
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
                app.send(id, CommandKind::Navigate(value.as_bytes().to_vec()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hosted_view_keeps_the_whole_page_after_show_all() {
        crate::with_gtk(|| {
            let container = placeholder(|| {});
            let view = gtk::Label::new(None);
            host(&container, view.upcast_ref());
            container.show_all();
            let visible: Vec<_> = container
                .children()
                .into_iter()
                .filter(|child| child.is_visible())
                .collect();
            assert_eq!(visible, [view.upcast::<gtk::Widget>()]);
        });
    }
}
