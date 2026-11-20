//! Bookmarking the tabs on screen: which tabs can be bookmarked, the window's copy of the list, and the
//! toggle behind the tab star and Ctrl+Shift+D. The list lives in the user's `bookmarks.yaml`, shared by every
//! window; each window re-reads it when it gains focus, so there is no watching or polling.
use super::App;
use std::rc::Rc;
use worminal_workspace::bookmarks::{Bookmark, Bookmarks, Target};

fn store() -> Bookmarks {
    Bookmarks::new(worminal_config::bookmarks_path())
}

/// The saved list, empty when there is none or it is damaged (toggling reports a damaged file).
pub(super) fn load() -> Vec<Bookmark> {
    store().list().unwrap_or_default()
}

impl App {
    /// What bookmarking this tab would save: a terminal with a known directory (and its host, when the
    /// daemon is remote), or a web page with a real URL. Anything else cannot be bookmarked.
    pub(super) fn bookmark_target(&self, id: u32) -> Option<Target> {
        let master = match self.master_of(id) {
            Some(master) => Some(master.into_string().ok()?),
            None => None,
        };
        let pages = self.pages.borrow();
        let page = pages.iter().find(|page| page.id == id)?;
        let target = if page.browser {
            if page.url == "about:blank" {
                return None;
            }
            Target::Web {
                url: page.url.clone(),
            }
        } else {
            Target::Terminal {
                directory: page.directory.to_str()?.to_owned(),
                master,
            }
        };
        let bookmark = Bookmark {
            target,
            title: None,
        };
        bookmark.validate().ok()?;
        Some(bookmark.target)
    }
    /// Whether the tab can be bookmarked, and whether it is.
    pub(super) fn bookmark_state(&self, id: u32) -> (bool, bool) {
        let Some(target) = self.bookmark_target(id) else {
            return (false, false);
        };
        let starred = self
            .bookmarks
            .borrow()
            .iter()
            .any(|bookmark| bookmark.target == target);
        (true, starred)
    }
    /// Read the shared list again and redraw the stars if another window changed it.
    pub(super) fn reload_bookmarks(&self) {
        let Ok(list) = store().list() else { return };
        if *self.bookmarks.borrow() != list {
            *self.bookmarks.borrow_mut() = list;
            self.refresh_tabs();
        }
    }
    /// Add the tab's bookmark or remove it.
    pub(super) fn toggle_bookmark(self: &Rc<Self>, id: u32) {
        let Some(target) = self.bookmark_target(id) else {
            self.message(Some("This tab has no directory or page to bookmark."));
            return;
        };
        // A page is named by its title; a terminal by its directory, which the strip already shows.
        let title = match &target {
            Target::Web { .. } => self
                .pages
                .borrow()
                .iter()
                .find(|page| page.id == id)
                .map(|page| page.title.clone())
                .filter(|title| !title.is_empty() && title.len() <= 256),
            Target::Terminal { .. } => None,
        };
        match store().toggle(Bookmark { target, title }) {
            Ok(now) => {
                self.message(Some(if now {
                    "Bookmarked."
                } else {
                    "Bookmark removed."
                }));
                *self.bookmarks.borrow_mut() = load();
                self.refresh_tabs();
            }
            Err(error) => self.message(Some(&error.to_string())),
        }
    }
}
