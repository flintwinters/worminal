//! Opening tabs: a new terminal beside the one being viewed, or one a bookmark names.
use super::{App, LINK_SHIFT};
use std::{ffi::OsString, path::PathBuf, rc::Rc};
use worminal_protocol::worp::{
    commands::{CommandKind, OpenMode},
    launch::Launch,
};

impl App {
    pub(super) fn new_tab(self: &Rc<Self>) {
        // A new tab opens on the host of the tab being viewed, in that tab's directory.
        let directory = self
            .pages
            .borrow()
            .iter()
            .find(|page| page.id == self.active.get())
            .map(|page| page.directory.clone())
            .filter(|directory| !directory.as_os_str().is_empty());
        self.open_terminal_on(self.active.get() >> LINK_SHIFT, directory);
    }
    /// A new shell on one link's daemon, in `directory` when given.
    fn open_terminal_on(self: &Rc<Self>, index: u32, directory: Option<PathBuf>) {
        let mut config = self.config_for(index);
        config.command.clear();
        config.title = "Worminal".into();
        config.output.clear();
        config.line.clear();
        config.browser_url = None;
        if let Some(directory) = directory {
            config.cwd = directory;
        }
        match config.launch().and_then(|bytes| Launch::decode(&bytes)) {
            Ok(launch) => {
                if let Some(link) = self.link(index) {
                    link.pending_tabs
                        .set(link.pending_tabs.get().saturating_add(1));
                }
                self.send_link(
                    index,
                    0,
                    CommandKind::Open {
                        launch,
                        mode: OpenMode::Subscribe,
                    },
                );
            }
            Err(error) => self.message(Some(&error.to_string())),
        }
    }
    /// A bookmarked terminal: a fresh shell in its directory on its host. The window's own daemon, a host it
    /// already shows, or a new link to the host, as `--master` would add one.
    pub(super) fn open_bookmarked_terminal(
        self: &Rc<Self>,
        directory: String,
        master: Option<String>,
    ) {
        let directory = PathBuf::from(directory);
        let Some(master) = master.map(OsString::from) else {
            if self.config.master.is_none() {
                self.open_terminal_on(0, Some(directory));
            } else {
                self.message(Some(
                    "This window shows another host; open a local bookmark from a local window.",
                ));
            }
            return;
        };
        if let Some(index) = self.master_index(&master) {
            self.open_terminal_on(index, Some(directory));
            return;
        }
        let mut config = self.config.clone();
        config.master = Some(master);
        config.cwd = directory;
        config.attach = false;
        config.command.clear();
        config.title = "Worminal".into();
        config.output.clear();
        config.line.clear();
        config.browser_url = None;
        if let Err(error) = self.add_remote(config) {
            self.message(Some(&error.to_string()));
        }
    }
}
