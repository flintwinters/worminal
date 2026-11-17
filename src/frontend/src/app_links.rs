//! Per-window daemon links, routing and reconnection state.
use super::{window_state, App, Config, Connection, Link, Remote, LINK_SHIFT};
use gtk::{glib, prelude::*};
use std::{ffi::OsString, io, os::fd::AsRawFd, rc::Rc, time::Duration};
use worminal_protocol::worp::{
    commands::{Command, CommandKind, OpenMode},
    launch::Launch,
    v2_commands, v2_events,
};
use worminal_protocol::*;

impl App {
    pub(super) fn link(&self, index: u32) -> Option<Rc<Link>> {
        if index == 0 {
            return Some(self.primary.clone());
        }
        self.remotes
            .borrow()
            .iter()
            .find(|remote| remote.index == index)
            .map(|remote| remote.link.clone())
    }
    /// The launch configuration of one link: the window's own, or the one its `--master` was opened with.
    pub(super) fn links(&self) -> Vec<Rc<Link>> {
        std::iter::once(self.primary.clone())
            .filter(|_| !self.primary_detached.get())
            .chain(
                self.remotes
                    .borrow()
                    .iter()
                    .map(|remote| remote.link.clone()),
            )
            .collect()
    }
    pub(super) fn restore_remote(self: &Rc<Self>, index: u32, config: Config) -> io::Result<()> {
        let master = config
            .master
            .clone()
            .ok_or_else(|| io::Error::other("Missing remote host"))?;
        if self.link(index).is_some() {
            return Err(io::Error::other("Duplicate remote link"));
        }
        self.next_link.set(self.next_link.get().max(index + 1));
        self.remotes.borrow_mut().push(Rc::new(Remote {
            index,
            master,
            config,
            link: Link::new(),
        }));
        self.establish(index, false)
    }
    pub(super) fn config_for(&self, index: u32) -> Config {
        self.remotes
            .borrow()
            .iter()
            .find(|remote| remote.index == index)
            .map_or_else(|| self.config.clone(), |remote| remote.config.clone())
    }
    /// Daemon host of a tab, or `None` for this machine.
    pub(super) fn master_of(&self, id: u32) -> Option<OsString> {
        self.config_for(id >> LINK_SHIFT).master
    }
    pub(super) fn features(&self, id: u32) -> u32 {
        self.link(id >> LINK_SHIFT)
            .map_or(0, |link| link.features.get())
    }
    pub(super) fn establish(self: &Rc<Self>, index: u32, launch: bool) -> io::Result<()> {
        let config = self.config_for(index);
        let link = self
            .link(index)
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        let connection = Connection::new(
            config.master.as_deref(),
            self.replacing.get(),
            link.reconnect.borrow().is_none(),
        )?;
        if index == 0 {
            self.primary_detached.set(false);
        }
        *link.connection.borrow_mut() = Some(connection);
        link.initial_claim.set(true);
        self.watch(index, &link);

        // Keep disconnection feedback visible until a catalog confirms the SSH handshake.
        link.features.set(0);
        self.send_link(
            index,
            0,
            CommandKind::Capabilities(
                WIRE_FEATURE_COPY_ROWS
                    | WIRE_FEATURE_ROW_SPAN
                    | WIRE_FEATURE_FOCUSED_FRAMES
                    | WIRE_FEATURE_NOTIFICATIONS
                    | WIRE_FEATURE_ATTENTION
                    | WIRE_FEATURE_ROW_LINKS,
            ),
        );
        if launch && index == 0 && config.browser_url.is_some() {
            self.open_browser(config.browser_url.as_deref().unwrap());
        } else if launch {
            self.send_link(
                index,
                0,
                CommandKind::Open {
                    launch: Launch::decode(&config.launch()?)?,
                    mode: OpenMode::ReuseRestored,
                },
            );
        } else {
            self.send_link(index, 0, CommandKind::Subscribe);
        }
        // A disconnect discards the old stream's replies. Re-fence the new stream after
        // its catalog instead of waiting forever for an acknowledgement on a dead socket.
        // Only the fence is retried; accepted terminal input is never replayed.
        let retry_fence = link.update_fences.borrow_mut().reconnect();
        if retry_fence {
            self.send_link(index, 0, CommandKind::ServiceInfo);
        }
        if link.connection.borrow().is_none() {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        Ok(())
    }
    pub(super) fn watch(self: &Rc<Self>, index: u32, link: &Rc<Link>) {
        let fd = link
            .connection
            .borrow()
            .as_ref()
            .map(|connection| connection.socket.as_raw_fd());
        let Some(fd) = fd else { return };
        let weak = Rc::downgrade(self);
        let link_weak = Rc::downgrade(link);
        *link.reader.borrow_mut() = Some(glib::source::unix_fd_add_local(
            fd,
            glib::IOCondition::IN | glib::IOCondition::HUP | glib::IOCondition::ERR,
            move |_, _| {
                let (Some(app), Some(link)) = (weak.upgrade(), link_weak.upgrade()) else {
                    return glib::ControlFlow::Break;
                };
                let packets = link.connection.borrow_mut().as_mut().map(Connection::read);
                match packets {
                    Some(Ok(packets)) => {
                        for packet in packets {
                            // A catalog can retire this link while later packets are still queued.
                            if app.closing.get() || link.connection.borrow().is_none() {
                                return glib::ControlFlow::Break;
                            }
                            if let Err(error) =
                                v2_events::decode(packet).and_then(|event| match event {
                                    Some(event) => app.event(index, event),
                                    None => Ok(()),
                                })
                            {
                                app.lost(index, &error.to_string());
                                return glib::ControlFlow::Break;
                            }
                        }
                        glib::ControlFlow::Continue
                    }
                    Some(Err(error)) => {
                        app.lost(index, &error.to_string());
                        glib::ControlFlow::Break
                    }
                    None => glib::ControlFlow::Break,
                }
            },
        ));
    }
    pub(super) fn update_barriers(self: &Rc<Self>) -> Vec<super::upgrade::UpdateBarrier> {
        let indexes = std::iter::once(0)
            .chain(self.remotes.borrow().iter().map(|remote| remote.index))
            .collect::<Vec<_>>();
        let mut barriers = Vec::new();
        for index in indexes {
            let Some(link) = self
                .link(index)
                .filter(|link| link.connection.borrow().is_some())
            else {
                continue;
            };
            let revision = link.update_fences.borrow_mut().request();
            barriers.push((link.clone(), revision));
            // The explicit catalog precedes a unique service-info reply on this stream.
            // That reply fences all earlier accepted input even when SSH is backpressured;
            // an unsolicited catalog alone is not a causal acknowledgement.
            self.send_link(index, 0, CommandKind::Subscribe);
            self.send_link(index, 0, CommandKind::ServiceInfo);
        }
        barriers
    }
    pub(super) fn pause_update_reads(&self) {
        for link in self.links() {
            if let Some(reader) = link.reader.borrow_mut().take() {
                reader.remove();
            }
        }
    }
    pub(super) fn resume_update_reads(self: &Rc<Self>) {
        for index in
            std::iter::once(0).chain(self.remotes.borrow().iter().map(|remote| remote.index))
        {
            let Some(link) = self.link(index) else {
                continue;
            };
            if link.reader.borrow().is_none() {
                self.watch(index, &link);
            }
        }
    }
    pub(super) fn clear_connection(&self) {
        self.primary.clear();
        for remote in self.remotes.borrow().iter() {
            remote.link.clear();
        }
    }
    /// Show another daemon's tabs in this window. `--master` hosts already present are only raised.
    pub(super) fn add_remote(self: &Rc<Self>, config: Config) -> io::Result<()> {
        let master = config.master.clone().expect("remote launch has a master");
        let known = if self.config.master.as_ref() == Some(&master) {
            Some(0)
        } else {
            self.remotes
                .borrow()
                .iter()
                .find(|remote| remote.master == master)
                .map(|remote| remote.index)
        };
        if let Some(index) = known {
            let link = self.link(index).unwrap();
            if !link.settled() {
                // An explicit launch retries authentication; background retries never prompt.
                link.clear();
                let launch = !config.attach
                    && !self
                        .pages
                        .borrow()
                        .iter()
                        .any(|page| page.id >> LINK_SHIFT == index);
                let reattaching_primary = index == 0 && self.primary_detached.get();
                self.establish(index, launch)?;
                if reattaching_primary {
                    // An explicit reattachment selects the returning host when its catalog arrives.
                    self.active.set(0);
                }
            }
        } else {
            let index = self.next_link.get();
            if index >= 1 << (32 - LINK_SHIFT) {
                return Err(io::Error::other("too many remote hosts in one window"));
            }
            self.next_link.set(index + 1);
            let launch = !config.attach;
            self.remotes.borrow_mut().push(Rc::new(Remote {
                index,
                master,
                config,
                link: Link::new(),
            }));
            if let Err(error) = self.establish(index, launch) {
                self.drop_remote(index);
                return Err(error);
            }
        }
        self.window.present();
        Ok(())
    }
    fn master_index(&self, master: &std::ffi::OsStr) -> Option<u32> {
        if self.config.master.as_deref() == Some(master) && !self.primary_detached.get() {
            Some(0)
        } else {
            self.remotes
                .borrow()
                .iter()
                .find(|remote| remote.master == master)
                .map(|remote| remote.index)
        }
    }
    /// Close each catalogued daemon tab once, even when multiple windows mirror the master.
    /// Keep links alive until normal catalog updates acknowledge removal; detaching here
    /// would discard queued closes under SSH backpressure.
    pub(super) fn close_master(master: &std::ffi::OsStr) -> io::Result<()> {
        let apps = super::WINDOWS.with(|windows| windows.borrow().clone());
        let mut targets = std::collections::BTreeMap::new();
        for app in apps.into_iter().filter(|app| !app.closing.get()) {
            let Some(index) = app.master_index(master) else {
                continue;
            };
            let link = app.link(index).unwrap();
            if link.initial_claim.get() || link.connection.borrow().is_none() {
                continue;
            }
            for id in link.catalog_tabs.borrow().iter().copied() {
                targets
                    .entry(id)
                    .or_insert((app.clone(), id | index << LINK_SHIFT));
            }
        }
        if targets.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "No connected tabs for master",
            ));
        }
        for (app, id) in targets.into_values() {
            app.try_send_link(
                id >> LINK_SHIFT,
                id & ((1 << LINK_SHIFT) - 1),
                CommandKind::Close { receipt: false },
            )?;
        }
        Ok(())
    }
    /// Detachment imports an empty view catalog, never a daemon Close command.
    pub(super) fn detach_master(master: &std::ffi::OsStr) -> io::Result<()> {
        let apps = super::WINDOWS.with(|windows| windows.borrow().clone());
        let mut found = false;
        for app in apps.into_iter().filter(|app| !app.closing.get()) {
            let Some(index) = app.master_index(master) else {
                continue;
            };
            found = true;
            let link = app.link(index).unwrap();
            // Stop readers, writers and reconnect timers before changing widgets: GTK can reenter.
            link.clear();
            link.pending_tabs.set(0);
            link.requested.set(0);
            if index == 0 {
                app.primary_detached.set(true);
            }
            app.catalog(
                index,
                &worminal_protocol::worp::events::Catalog {
                    closed: 0,
                    fallback: 0,
                    tabs: Vec::new(),
                },
            )?;
            app.close_empty_window();
        }
        if found {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Master is not connected",
            ))
        }
    }
    pub(super) fn close_empty_window(&self) {
        // An empty view may still be waiting for another host's first catalog or new tab.
        if self.pages.borrow().is_empty() && self.links().iter().all(|link| link.settled()) {
            self.window.close();
        }
    }
    pub(super) fn drop_remote(&self, index: u32) {
        if let Some(link) = self.link(index).filter(|_| index != 0) {
            link.clear();
        }
        self.remotes
            .borrow_mut()
            .retain(|remote| remote.index != index);
    }
    pub(super) fn lost(self: &Rc<Self>, index: u32, error: &str) {
        if self.closing.get() {
            return;
        }
        let Some(link) = self.link(index) else { return };
        link.clear();
        link.pending_tabs.set(0);
        for page in self.pages.borrow().iter() {
            if page.id >> LINK_SHIFT == index {
                if let Some(terminal) = &page.terminal {
                    terminal.release();
                }
            }
        }
        self.message(Some(&format!("Disconnected: {error}. Reconnecting…")));
        window_state::changed();
        if link.reconnect.borrow().is_none() {
            let weak = Rc::downgrade(self);
            let link_weak = Rc::downgrade(&link);
            let delay = link.retry_delay.get();
            link.retry_delay
                .set((delay * 2).min(Duration::from_secs(30)));
            *link.reconnect.borrow_mut() = Some(glib::timeout_add_local(delay, move || {
                let (Some(app), Some(link)) = (weak.upgrade(), link_weak.upgrade()) else {
                    return glib::ControlFlow::Break;
                };
                if app.establish(index, false).is_ok() {
                    link.reconnect.borrow_mut().take();
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            }));
        }
    }
    pub(super) fn send(self: &Rc<Self>, tab: u32, kind: CommandKind) {
        self.send_link(tab >> LINK_SHIFT, tab & ((1 << LINK_SHIFT) - 1), kind);
    }
    pub(super) fn send_link(self: &Rc<Self>, index: u32, tab: u32, kind: CommandKind) {
        if let Err(error) = self.try_send_link(index, tab, kind) {
            self.lost(index, &error.to_string());
        }
    }
    fn try_send_link(self: &Rc<Self>, index: u32, tab: u32, kind: CommandKind) -> io::Result<()> {
        let link = self
            .link(index)
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotConnected))?;
        let packet = v2_commands::encode(&Command { tab, kind })?;
        link.connection
            .borrow_mut()
            .as_mut()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotConnected))?
            .queue(packet)?;
        let fd = link
            .connection
            .borrow()
            .as_ref()
            .filter(|connection| !connection.output.is_empty())
            .map(|connection| connection.socket.as_raw_fd());
        if link.writer.borrow().is_none() {
            if let Some(fd) = fd {
                let weak = Rc::downgrade(self);
                let link_weak = Rc::downgrade(&link);
                *link.writer.borrow_mut() = Some(glib::source::unix_fd_add_local(
                    fd,
                    glib::IOCondition::OUT | glib::IOCondition::HUP | glib::IOCondition::ERR,
                    move |_, _| {
                        let (Some(app), Some(link)) = (weak.upgrade(), link_weak.upgrade()) else {
                            return glib::ControlFlow::Break;
                        };
                        let result = link.connection.borrow_mut().as_mut().map(|connection| {
                            connection.flush().map(|_| connection.output.is_empty())
                        });
                        match result {
                            Some(Ok(false)) => glib::ControlFlow::Continue,
                            Some(Err(error)) => {
                                app.lost(index, &error.to_string());
                                glib::ControlFlow::Break
                            }
                            _ => {
                                link.writer.borrow_mut().take();
                                glib::ControlFlow::Break
                            }
                        }
                    },
                ));
            }
        }
        Ok(())
    }
}
