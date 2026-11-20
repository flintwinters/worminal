//! Opening tabs: a new terminal beside the one being viewed.
use super::{App, LINK_SHIFT};
use std::rc::Rc;
use worminal_protocol::worp::{
    commands::{CommandKind, OpenMode},
    launch::Launch,
};

impl App {
    pub(super) fn new_tab(self: &Rc<Self>) {
        // A new tab opens on the host of the tab being viewed.
        let index = self.active.get() >> LINK_SHIFT;
        let mut config = self.config_for(index);
        config.command.clear();
        config.title = "Worminal".into();
        config.output.clear();
        config.line.clear();
        config.browser_url = None;
        if let Some(page) = self
            .pages
            .borrow()
            .iter()
            .find(|page| page.id == self.active.get())
        {
            if !page.directory.as_os_str().is_empty() {
                config.cwd = page.directory.clone();
            }
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
}
