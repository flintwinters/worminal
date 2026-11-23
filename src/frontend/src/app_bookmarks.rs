//! Bookmarking the tabs on screen: which tabs can be bookmarked, the window's copy of the list, and the
//! toggle behind the tab star and Ctrl+Shift+D. The list lives in the user's `bookmarks.yaml`, shared by every
//! window; each window re-reads it when it gains focus, so there is no watching or polling.
use super::App;
use std::{path::Path, rc::Rc};
use worminal_workspace::bookmarks::{Bookmark, Bookmarks, Target};

pub(super) fn store() -> Bookmarks {
    Bookmarks::new(worminal_config::bookmarks_path())
}

/// The saved list, empty when there is none or it is damaged (toggling reports a damaged file).
pub(super) fn load() -> Vec<Bookmark> {
    store().list().unwrap_or_default()
}

/// What bookmarking a tab saves, or `None` when it has nothing to save.
///
/// A terminal needs a known UTF-8 directory. While an SSH client is in its foreground the directory is the *local*
/// shell's, not where the user is: bookmarking it would save the wrong place and light up every tab that shares
/// that directory, so such a tab is not bookmarkable. A web page needs a real URL.
fn target_of(
    browser: bool,
    url: &str,
    directory: &Path,
    ssh: bool,
    master: Option<String>,
) -> Option<Target> {
    let target = if browser {
        if url == "about:blank" {
            return None;
        }
        Target::Web {
            url: url.to_owned(),
        }
    } else {
        if ssh {
            return None;
        }
        Target::Terminal {
            directory: directory.to_str()?.to_owned(),
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

impl App {
    /// What bookmarking this tab would save: a terminal with a known directory (and its host, when the
    /// daemon is remote), or a web page with a real URL. Anything else cannot be bookmarked.
    pub(super) fn bookmark_target(&self, id: u32) -> Option<Target> {
        let master = self.master_of(id);
        let pages = self.pages.borrow();
        let page = pages.iter().find(|page| page.id == id)?;
        target_of(
            page.browser,
            &page.url,
            &page.directory,
            page.ssh_user.is_some(),
            master,
        )
    }
    /// Whether the tab shows a star, and whether it is filled. A hollow star only appears with
    /// `worminal.unbookmarked_stars`; Ctrl+Shift+D and `worminal --bookmark` work either way.
    pub(super) fn bookmark_state(&self, id: u32) -> (bool, bool) {
        let Some(target) = self.bookmark_target(id) else {
            return (false, false);
        };
        let starred = self
            .bookmarks
            .borrow()
            .iter()
            .any(|bookmark| bookmark.target == target);
        (
            starred || worminal_config::get().unbookmarked_stars,
            starred,
        )
    }
    /// Read the shared list again and redraw the stars if another window changed it.
    pub(super) fn reload_bookmarks(&self) {
        let Ok(list) = store().list() else { return };
        if *self.bookmarks.borrow() != list {
            *self.bookmarks.borrow_mut() = list;
            self.refresh_tabs();
            self.refresh_panel();
        }
    }
    /// `worminal --bookmark`: the launcher runs in a tab's shell, so its working directory is that tab's.
    /// Adding (never toggling) keeps a repeated command harmless. Every window redraws its stars.
    pub(super) fn bookmark_directory(directory: &Path) -> std::io::Result<()> {
        let target = target_of(false, "", directory, false, None).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{} cannot be bookmarked.", directory.display()),
            )
        })?;
        store().add(Bookmark {
            target,
            title: None,
        })?;
        for app in super::WINDOWS.with(|windows| windows.borrow().clone()) {
            app.reload_bookmarks();
        }
        Ok(())
    }
    /// Add the tab's bookmark or remove it.
    pub(super) fn toggle_bookmark(self: &Rc<Self>, id: u32) {
        let Some(target) = self.bookmark_target(id) else {
            // Say why, but only on a failure: a message under the panes takes a row from the terminal and makes
            // the screen jump, so success is shown by the star alone.
            let ssh = self
                .pages
                .borrow()
                .iter()
                .any(|page| page.id == id && page.ssh_user.is_some());
            self.message(Some(if ssh {
                "This tab is running ssh: its directory is on the other host and is not known here."
            } else {
                "This tab has no directory or page to bookmark."
            }));
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
            // A tab's target is never a Host: a tab running ssh is not bookmarkable.
            Target::Terminal { .. } | Target::Host { .. } => None,
        };
        match store().toggle(Bookmark { target, title }) {
            Ok(_) => self.reload_bookmarks(),
            Err(error) => self.message(Some(&error.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::target_of;
    use std::path::Path;
    use worminal_workspace::bookmarks::Target;

    #[test]
    fn a_terminal_saves_its_directory_and_host_but_not_while_ssh_is_in_front() {
        let here = Path::new("/var/opt/worminal");
        assert_eq!(
            target_of(false, "", here, false, None),
            Some(Target::Terminal {
                directory: "/var/opt/worminal".into(),
                master: None
            })
        );
        assert_eq!(
            target_of(false, "", here, false, Some("me@build".into())),
            Some(Target::Terminal {
                directory: "/var/opt/worminal".into(),
                master: Some("me@build".into())
            })
        );
        // The directory of a tab running ssh is the local shell's, not the remote one it shows.
        assert_eq!(target_of(false, "", here, true, None), None);
        assert_eq!(
            target_of(false, "", here, true, Some("me@build".into())),
            None
        );
        // No directory known yet, or one that is not text.
        assert_eq!(target_of(false, "", Path::new(""), false, None), None);
        use std::os::unix::ffi::OsStrExt;
        assert_eq!(
            target_of(
                false,
                "",
                Path::new(std::ffi::OsStr::from_bytes(b"/tmp/\xff")),
                false,
                None
            ),
            None
        );
    }

    #[test]
    fn a_page_saves_its_url_unless_it_is_blank() {
        let none = Path::new("");
        assert_eq!(
            target_of(true, "https://example.org", none, false, None),
            Some(Target::Web {
                url: "https://example.org".into()
            })
        );
        assert_eq!(target_of(true, "about:blank", none, false, None), None);
        assert_eq!(target_of(true, "", none, false, None), None);
        assert_eq!(target_of(true, "has space", none, false, None), None);
    }
}
