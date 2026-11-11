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
        let connection = Connection::new(config.master.as_deref(), self.replacing.get())?;
        *link.connection.borrow_mut() = Some(connection);
        if let (Some(master), false) = (config.master.as_deref(), self.replacing.get()) {
            // Best effort: a failed helper must never affect the connection itself.
            let _ = crate::startup::sync_versions(master);
        }
        link.initial_claim.set(true);
        self.watch(index, &link);

        self.message(None);
        link.features.set(0);
        self.send_link(
            index,
            0,
            CommandKind::Capabilities(
                WIRE_FEATURE_COPY_ROWS
                    | WIRE_FEATURE_ROW_SPAN
                    | WIRE_FEATURE_FOCUSED_FRAMES
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
    pub(super) fn clear_connection(&self) {
        self.primary.clear();
        for remote in self.remotes.borrow().iter() {
            remote.link.clear();
        }
    }
    /// Show another daemon's tabs in this window. `--master` hosts already present are only raised.
    pub(super) fn add_remote(self: &Rc<Self>, config: Config) -> io::Result<()> {
        let master = config.master.clone().expect("remote launch has a master");
        let known = self.config.master.as_ref() == Some(&master)
            || self
                .remotes
                .borrow()
                .iter()
                .any(|remote| remote.master == master);
        if !known {
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
            *link.reconnect.borrow_mut() = Some(glib::timeout_add_local(
                Duration::from_millis(250),
                move || {
                    let (Some(app), Some(link)) = (weak.upgrade(), link_weak.upgrade()) else {
                        return glib::ControlFlow::Break;
                    };
                    if app.establish(index, false).is_ok() {
                        link.reconnect.borrow_mut().take();
                        glib::ControlFlow::Break
                    } else {
                        glib::ControlFlow::Continue
                    }
                },
            ));
        }
    }
    pub(super) fn send(self: &Rc<Self>, tab: u32, kind: CommandKind) {
        self.send_link(tab >> LINK_SHIFT, tab & ((1 << LINK_SHIFT) - 1), kind);
    }
    pub(super) fn send_link(self: &Rc<Self>, index: u32, tab: u32, kind: CommandKind) {
        let Some(link) = self.link(index) else { return };
        let result = v2_commands::encode(&Command { tab, kind }).and_then(|packet| {
            link.connection
                .borrow_mut()
                .as_mut()
                .map(|connection| connection.queue(packet))
                .transpose()
        });
        if let Err(error) = result {
            self.lost(index, &error.to_string());
            return;
        }
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
    }
}
