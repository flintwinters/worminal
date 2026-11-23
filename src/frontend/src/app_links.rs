//! The window's daemon link, routing and reconnection state.
use super::{window_state, App, Connection, Link, Record};
use gtk::glib;
use std::{io, os::fd::AsRawFd, rc::Rc, time::Duration};
use worminal_protocol::worp::{
    commands::{Command, CommandKind, MasterOperation, OpenMode},
    launch::Launch,
    v2_commands, v2_events,
};
use worminal_protocol::*;

/// First reconnect delay; a link whose delay has grown has not recovered since it was lost.
pub(super) const RETRY_INITIAL: Duration = Duration::from_millis(250);

impl Link {
    /// Nothing more to wait for: settled, or lost so that no reply can ever settle it. Updates
    /// wait on this, never on an unreachable peer.
    pub(super) fn quiet(&self) -> bool {
        self.down() || self.settled()
    }
    /// Lost and not yet recovered. Only a catalog resets the backoff, so this holds through the
    /// brief reconnect attempts of an unreachable peer too.
    pub(super) fn down(&self) -> bool {
        self.retry_delay.get() != RETRY_INITIAL
    }
}

impl App {
    /// The master daemon that owns tab `id`, or `None` for this machine.
    pub(super) fn master_of(&self, id: u32) -> Option<String> {
        self.pages
            .borrow()
            .iter()
            .find(|page| page.id == id)
            .and_then(|page| page.host.clone())
    }
    pub(super) fn features(&self) -> u32 {
        self.primary.features.get()
    }
    /// Ask the daemon to show `master`'s tabs, prompting for a password if SSH needs one, and open `launch`
    /// there, which is selected when created. Without one, the first of its tabs is selected once listed.
    pub(super) fn attach_master(
        self: &Rc<Self>,
        master: &std::ffi::OsStr,
        launch: Option<Box<Launch>>,
    ) {
        let host = master.to_string_lossy().into_owned();
        if launch.is_some() {
            let link = &self.primary;
            link.pending_tabs
                .set(link.pending_tabs.get().saturating_add(1));
        } else {
            *self.awaited.borrow_mut() = Some(host.clone());
        }
        self.send(
            0,
            CommandKind::Master {
                operation: MasterOperation::Attach,
                host: host.into_bytes(),
                launch,
            },
        );
        // The daemon may already list the master's tabs; its reply catalog follows them then.
        self.send(0, CommandKind::Subscribe);
    }
    /// The first tab of the master the user attached without opening a terminal, once a catalog lists it.
    pub(super) fn follow_master(&self, records: &[Record]) -> Option<u32> {
        let id = {
            let awaited = self.awaited.borrow();
            let host = awaited.as_ref()?;
            records
                .iter()
                .find(|record| record.host.as_ref() == Some(host))?
                .id
        };
        self.awaited.borrow_mut().take();
        Some(id)
    }
    pub(super) fn establish(self: &Rc<Self>, launch: bool) -> io::Result<()> {
        let link = self.primary.clone();
        let connection = Connection::new()?;
        *link.connection.borrow_mut() = Some(connection);
        link.initial_claim.set(true);
        self.watch();

        // Keep disconnection feedback visible until a catalog confirms the connection.
        link.features.set(0);
        self.send(
            0,
            CommandKind::Capabilities(
                WIRE_FEATURE_COPY_ROWS
                    | WIRE_FEATURE_ROW_SPAN
                    | WIRE_FEATURE_FOCUSED_FRAMES
                    | WIRE_FEATURE_NOTIFICATIONS
                    | WIRE_FEATURE_ATTENTION
                    | WIRE_FEATURE_ROW_LINKS
                    | WIRE_FEATURE_HOSTS,
            ),
        );
        if launch && self.config.browser_url.is_some() {
            self.open_browser(self.config.browser_url.as_deref().unwrap());
        } else if launch {
            self.send(
                0,
                CommandKind::Open {
                    launch: Launch::decode(&self.config.launch()?)?,
                    mode: OpenMode::ReuseRestored,
                },
            );
        } else {
            self.send(0, CommandKind::Subscribe);
        }
        // A disconnect discards the old stream's replies. Re-fence the new stream after
        // its catalog instead of waiting forever for an acknowledgement on a dead socket.
        // Only the fence is retried; accepted terminal input is never replayed.
        let retry_fence = link.update_fences.borrow_mut().reconnect();
        if retry_fence {
            self.send(0, CommandKind::ServiceInfo);
        }
        if link.connection.borrow().is_none() {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        Ok(())
    }
    pub(super) fn watch(self: &Rc<Self>) {
        let link = &self.primary;
        let fd = link
            .connection
            .borrow()
            .as_ref()
            .map(|connection| connection.socket.as_raw_fd());
        let Some(fd) = fd else { return };
        let weak = Rc::downgrade(self);
        *link.reader.borrow_mut() = Some(glib::source::unix_fd_add_local(
            fd,
            glib::IOCondition::IN | glib::IOCondition::HUP | glib::IOCondition::ERR,
            move |_, _| {
                let Some(app) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                let link = app.primary.clone();
                let packets = link.connection.borrow_mut().as_mut().map(Connection::read);
                match packets {
                    Some(Ok(packets)) => {
                        for packet in packets {
                            // A catalog can close this window while later packets are still queued.
                            if app.closing.get() || link.connection.borrow().is_none() {
                                return glib::ControlFlow::Break;
                            }
                            if let Err(error) =
                                v2_events::decode(packet).and_then(|event| match event {
                                    Some(event) => app.event(event),
                                    None => Ok(()),
                                })
                            {
                                app.lost(&error.to_string());
                                return glib::ControlFlow::Break;
                            }
                        }
                        glib::ControlFlow::Continue
                    }
                    Some(Err(error)) => {
                        app.lost(&error.to_string());
                        glib::ControlFlow::Break
                    }
                    None => glib::ControlFlow::Break,
                }
            },
        ));
    }
    pub(super) fn update_barriers(self: &Rc<Self>) -> Vec<super::upgrade::UpdateBarrier> {
        let link = self.primary.clone();
        if link.connection.borrow().is_none() {
            return Vec::new();
        }
        let revision = link.update_fences.borrow_mut().request();
        // The explicit catalog precedes a unique service-info reply on this stream.
        // That reply fences all earlier accepted input even when the socket is backpressured;
        // an unsolicited catalog alone is not a causal acknowledgement.
        self.send(0, CommandKind::Subscribe);
        self.send(0, CommandKind::ServiceInfo);
        vec![(link, revision)]
    }
    pub(super) fn pause_update_reads(&self) {
        if let Some(reader) = self.primary.reader.borrow_mut().take() {
            reader.remove();
        }
    }
    pub(super) fn resume_update_reads(self: &Rc<Self>) {
        if self.primary.reader.borrow().is_none() {
            self.watch();
        }
    }
    pub(super) fn clear_connection(&self) {
        self.primary.clear();
    }
    pub(super) fn clear_message(&self) {
        if !self.primary.down() {
            self.message(None);
        }
    }
    pub(super) fn lost(self: &Rc<Self>, error: &str) {
        if self.closing.get() {
            return;
        }
        let link = self.primary.clone();
        link.clear();
        link.pending_tabs.set(0);
        for page in self.pages.borrow().iter() {
            if let Some(terminal) = &page.terminal {
                terminal.release();
            }
        }
        self.message(Some(&format!(
            "Disconnected: {}. Reconnecting…",
            error.trim_end_matches('.')
        )));
        window_state::changed();
        if link.reconnect.borrow().is_none() {
            let weak = Rc::downgrade(self);
            let delay = link.retry_delay.get();
            link.retry_delay
                .set((delay * 2).min(Duration::from_secs(30)));
            *link.reconnect.borrow_mut() = Some(glib::timeout_add_local(delay, move || {
                let Some(app) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                if app.establish(false).is_ok() {
                    app.primary.reconnect.borrow_mut().take();
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            }));
        }
    }
    pub(super) fn send(self: &Rc<Self>, tab: u32, kind: CommandKind) {
        if let Err(error) = self.try_send(tab, kind) {
            self.lost(&error.to_string());
        }
    }
    fn try_send(self: &Rc<Self>, tab: u32, kind: CommandKind) -> io::Result<()> {
        let link = self.primary.clone();
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
                *link.writer.borrow_mut() = Some(glib::source::unix_fd_add_local(
                    fd,
                    glib::IOCondition::OUT | glib::IOCondition::HUP | glib::IOCondition::ERR,
                    move |_, _| {
                        let Some(app) = weak.upgrade() else {
                            return glib::ControlFlow::Break;
                        };
                        let link = app.primary.clone();
                        let result = link.connection.borrow_mut().as_mut().map(|connection| {
                            connection.flush().map(|_| connection.output.is_empty())
                        });
                        match result {
                            Some(Ok(false)) => glib::ControlFlow::Continue,
                            Some(Err(error)) => {
                                app.lost(&error.to_string());
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
