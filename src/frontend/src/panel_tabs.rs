//! What the side panel lists: its tabs, their rows and what pressing a row asks the window to do. Nothing here
//! touches GTK, so every rule is unit-tested; `panel.rs` draws and drives whatever implements `PanelTab`.
use worminal_workspace::bookmarks::{Bookmark, Target};

/// What a panel tab asks the window to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelAction {
    OpenTerminal {
        directory: String,
        master: Option<String>,
    },
    OpenUrl(String),
    /// A new tab that runs ssh to this endpoint.
    OpenHost(String),
    AddBookmark(Bookmark),
    RemoveBookmark(Target),
}
/// What the user pressed on a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelKey {
    Open,
    Remove,
}

/// One tab of the panel.
pub trait PanelTab {
    fn title(&self) -> &str;
    /// One line per row.
    fn rows(&self) -> Vec<String>;
    /// Shown instead of rows when there are none.
    fn empty(&self) -> &str;
    fn press(&self, key: PanelKey, row: usize) -> Option<PanelAction>;
    /// What pressing `a` asks the user to type, or `None` when this tab has nothing to add.
    fn add_prompt(&self) -> Option<&str> {
        None
    }
    /// What the typed text adds, or why it cannot be added.
    fn add(&self, _text: &str) -> Result<PanelAction, String> {
        Err("Nothing can be added here.".into())
    }
}

/// Saved terminal tabs and web pages.
pub struct BookmarksTab {
    items: Vec<Bookmark>,
    home: Option<String>,
}
impl BookmarksTab {
    pub fn new(items: Vec<Bookmark>, home: Option<String>) -> Self {
        Self { items, home }
    }
}
/// `~` for the user's home, so local rows stay short.
fn tilde(path: &str, home: Option<&str>) -> String {
    match home.filter(|home| !home.is_empty() && *home != "/") {
        Some(home) if path == home => "~".into(),
        Some(home) => path
            .strip_prefix(home)
            .filter(|rest| rest.starts_with('/'))
            .map_or_else(|| path.to_owned(), |rest| format!("~{rest}")),
        None => path.to_owned(),
    }
}
fn row_label(bookmark: &Bookmark, home: Option<&str>) -> String {
    match &bookmark.target {
        Target::Terminal {
            directory,
            master: Some(master),
        } => format!("{master}:{directory}"),
        Target::Terminal { directory, .. } => tilde(directory, home),
        Target::Web { url } => bookmark.title.clone().unwrap_or_else(|| url.clone()),
        Target::Host { endpoint } => bookmark
            .title
            .clone()
            .unwrap_or_else(|| format!("ssh {endpoint}")),
    }
}
impl PanelTab for BookmarksTab {
    fn title(&self) -> &str {
        "Bookmarks"
    }
    fn rows(&self) -> Vec<String> {
        self.items
            .iter()
            .map(|bookmark| row_label(bookmark, self.home.as_deref()))
            .collect()
    }
    fn empty(&self) -> &str {
        "No bookmarks yet. Ctrl+Shift+D bookmarks the current tab; a adds a server."
    }
    fn add_prompt(&self) -> Option<&str> {
        Some("Add server (user@host)")
    }
    fn add(&self, text: &str) -> Result<PanelAction, String> {
        let bookmark = Bookmark {
            target: Target::Host {
                endpoint: text.trim().to_owned(),
            },
            title: None,
        };
        bookmark
            .validate()
            .map_err(|_| "Not a server: use host or user@host.".to_owned())?;
        Ok(PanelAction::AddBookmark(bookmark))
    }
    fn press(&self, key: PanelKey, row: usize) -> Option<PanelAction> {
        let bookmark = self.items.get(row)?;
        Some(match (key, &bookmark.target) {
            (PanelKey::Remove, target) => PanelAction::RemoveBookmark(target.clone()),
            (PanelKey::Open, Target::Terminal { directory, master }) => PanelAction::OpenTerminal {
                directory: directory.clone(),
                master: master.clone(),
            },
            (PanelKey::Open, Target::Web { url }) => PanelAction::OpenUrl(url.clone()),
            (PanelKey::Open, Target::Host { endpoint }) => PanelAction::OpenHost(endpoint.clone()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn terminal(directory: &str, master: Option<&str>) -> Bookmark {
        Bookmark {
            target: Target::Terminal {
                directory: directory.into(),
                master: master.map(Into::into),
            },
            title: None,
        }
    }
    fn web(url: &str, title: Option<&str>) -> Bookmark {
        Bookmark {
            target: Target::Web { url: url.into() },
            title: title.map(Into::into),
        }
    }
    #[test]
    fn rows_abbreviate_home_and_name_the_host_and_page() {
        let tab = BookmarksTab::new(
            vec![
                terminal("/home/me", None),
                terminal("/home/me/work", None),
                terminal("/home/mellow", None),
                terminal("/srv/app", Some("me@build")),
                web("https://example.org", Some("Example")),
                web("https://example.org/x", None),
            ],
            Some("/home/me".into()),
        );
        assert_eq!(
            tab.rows(),
            [
                "~",
                "~/work",
                "/home/mellow",
                "me@build:/srv/app",
                "Example",
                "https://example.org/x"
            ]
        );
    }

    #[test]
    fn pressing_a_bookmark_opens_or_removes_exactly_that_bookmark() {
        let tab = BookmarksTab::new(
            vec![
                terminal("/srv/app", Some("me@build")),
                web("https://example.org", None),
            ],
            None,
        );
        assert_eq!(
            tab.press(PanelKey::Open, 0),
            Some(PanelAction::OpenTerminal {
                directory: "/srv/app".into(),
                master: Some("me@build".into())
            })
        );
        assert_eq!(
            tab.press(PanelKey::Open, 1),
            Some(PanelAction::OpenUrl("https://example.org".into()))
        );
        assert_eq!(
            tab.press(PanelKey::Remove, 1),
            Some(PanelAction::RemoveBookmark(Target::Web {
                url: "https://example.org".into()
            }))
        );
        assert_eq!(tab.press(PanelKey::Open, 2), None);
    }

    fn host(endpoint: &str, title: Option<&str>) -> Bookmark {
        Bookmark {
            target: Target::Host {
                endpoint: endpoint.into(),
            },
            title: title.map(Into::into),
        }
    }

    #[test]
    fn a_server_row_says_ssh_unless_it_has_a_title_and_opens_as_a_host() {
        let tab = BookmarksTab::new(
            vec![
                host("digitize@Luna", None),
                host("10.0.0.7", Some("Build box")),
            ],
            None,
        );
        assert_eq!(tab.rows(), ["ssh digitize@Luna", "Build box"]);
        assert_eq!(
            tab.press(PanelKey::Open, 1),
            Some(PanelAction::OpenHost("10.0.0.7".into()))
        );
        assert_eq!(
            tab.press(PanelKey::Remove, 0),
            Some(PanelAction::RemoveBookmark(Target::Host {
                endpoint: "digitize@Luna".into()
            }))
        );
    }

    #[test]
    fn typed_servers_are_added_only_when_they_are_valid_endpoints() {
        let tab = BookmarksTab::new(Vec::new(), None);
        assert!(tab.add_prompt().is_some());
        assert_eq!(
            tab.add("  me@build  "),
            Ok(PanelAction::AddBookmark(host("me@build", None)))
        );
        assert_eq!(
            tab.add("[::1]"),
            Ok(PanelAction::AddBookmark(host("[::1]", None)))
        );
        for bad in [
            "",
            "   ",
            "a b",
            "-oProxyCommand=x",
            "host;reboot",
            "me@",
            "$(id)",
        ] {
            assert!(tab.add(bad).is_err(), "{bad:?} was accepted");
        }
        // A tab with nothing to add keeps the trait's defaults.
        struct Fixed;
        impl PanelTab for Fixed {
            fn title(&self) -> &str {
                "Fixed"
            }
            fn rows(&self) -> Vec<String> {
                Vec::new()
            }
            fn empty(&self) -> &str {
                ""
            }
            fn press(&self, _: PanelKey, _: usize) -> Option<PanelAction> {
                None
            }
        }
        assert!(Fixed.add_prompt().is_none());
        assert!(Fixed.add("me@build").is_err());
    }
}
