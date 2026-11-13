use crate::{
    process,
    service::Service,
    state::{core, ClientId},
};
use std::io;
use worminal_engine::{MODE_FOCUS, WORMINAL_PRINT_SCREEN, WORMINAL_TOGGLE_PRINTER};
use worminal_protocol::{
    worp::{
        commands::*,
        events::{Event, EventKind},
        v2_commands, Handler,
    },
    *,
};

impl Service {
    fn error(&mut self, client: ClientId, message: &[u8]) -> io::Result<()> {
        self.emit(
            client,
            Event {
                tab: 0,
                kind: EventKind::Error(message.to_vec()),
            },
        );
        Ok(())
    }

    pub fn message(&mut self, client: ClientId, packet: Packet) -> io::Result<()> {
        let unary = packet.kind == WIRE_WORP;
        let tab = packet.tab;
        let launch = matches!(packet.kind, WIRE_HELLO | WIRE_NEW);
        let replacement = packet.kind == WIRE_BROWSER_REPLACE;
        match v2_commands::decode(packet) {
            Ok(Some(command)) => self.command(client, command),
            Ok(None) => Ok(()),
            Err(error) if unary => {
                self.reply(client, tab, crate::api::failure(&error.to_string()));
                Ok(())
            }
            Err(error) if launch => Err(error),
            Err(_) if replacement => self.error(
                client,
                b"Invalid browser URL: supply one nonempty URL up to 65536 bytes.",
            ),
            Err(_) => Ok(()),
        }
    }

    pub fn reply(&mut self, client: ClientId, tab: u32, reply: worminal_protocol::worp::Reply) {
        let reply = if reply.json.len() + 4 + HEADER_BYTES > crate::state::CLIENT_QUEUE {
            crate::api::failure("Worp reply exceeds the stream queue limit.")
        } else {
            reply
        };
        self.emit(
            client,
            Event {
                tab,
                kind: EventKind::Reply(reply),
            },
        );
    }

    #[expect(clippy::too_many_lines, reason = "one arm per semantic Worp command")]
    pub fn command(&mut self, client: ClientId, command: Command) -> io::Result<()> {
        let Some(peer) = self.clients.get(&client) else {
            return Ok(());
        };
        let ready = peer.ready;
        let peer_pid = peer.pid;
        let now = self.now();
        match command.kind {
            CommandKind::Call(request) => {
                let reply = self.handle(request)?;
                self.reply(client, command.tab, reply);
                return Ok(());
            }
            CommandKind::Input {
                ref bytes,
                control: Control::Preserve,
            } => {
                let reply = self.input_operation(command.tab, bytes, None, now)?;
                self.reply(client, command.tab, reply);
                return Ok(());
            }
            CommandKind::Close { receipt: true } => {
                let reply =
                    self.handle(worminal_protocol::worp::Request::Close { tab: command.tab })?;
                self.reply(client, command.tab, reply);
                return Ok(());
            }
            CommandKind::ServiceInfo => {
                self.emit(
                    client,
                    Event {
                        tab: 0,
                        kind: EventKind::ServiceInfo(std::process::id()),
                    },
                );
                return Ok(());
            }
            CommandKind::Capabilities(features) => {
                if !ready && command.tab == 0 {
                    self.clients.get_mut(&client).unwrap().features = features
                        & (WIRE_FEATURE_COPY_ROWS
                            | WIRE_FEATURE_ROW_SPAN
                            | WIRE_FEATURE_FOCUSED_FRAMES
                            | WIRE_FEATURE_NOTIFICATIONS
                            | WIRE_FEATURE_ATTENTION
                            | WIRE_FEATURE_ROW_LINKS);
                    self.emit(
                        client,
                        Event {
                            tab: 0,
                            kind: EventKind::Capabilities(
                                WIRE_FEATURE_COPY_ROWS
                                    | WIRE_FEATURE_SELECT_ALL
                                    | WIRE_FEATURE_ROW_SPAN
                                    | WIRE_FEATURE_NOTIFICATIONS
                                    | WIRE_FEATURE_ATTENTION
                                    | WIRE_FEATURE_ROW_LINKS,
                            ),
                        },
                    );
                }
                return Ok(());
            }
            CommandKind::Stop => {
                self.running = false;
                return Ok(());
            }
            CommandKind::Transfer { outbound, token } => {
                return self.handoff_command(client, outbound, &token);
            }
            CommandKind::ReplaceBrowser(url) => {
                let session = process::session_of(peer_pid).unwrap_or(0);
                let index = self.tabs.iter_mut().position(|tab| {
                    tab.terminal
                        .as_mut()
                        .is_some_and(|terminal| terminal.pty.live_pid() == session && session > 0)
                });
                let Some(index) = index else {
                    return self.error(client, concat!("This shell is not in this workspace. ",
                        "Open worminal and run the command there; keep WORMINAL_SHARED_SOCKET_SCOPE unchanged.")
                        .as_bytes());
                };
                if url.is_empty() || url.len() > 65536 {
                    return Ok(());
                }
                if self.pending_replace.contains_key(&client) {
                    return Ok(());
                }
                let id = self.tabs[index].id;
                self.pending_replace.insert(client, (id, url));
                self.emit(
                    client,
                    Event {
                        tab: id,
                        kind: EventKind::CommandDone,
                    },
                );
                self.client_flush(client);
                return Ok(());
            }
            CommandKind::Subscribe => {
                self.restored_launch = false;
                self.clients.get_mut(&client).unwrap().ready = true;
                return self.send_catalog(client, now);
            }
            CommandKind::Browser { url, subscribe } => {
                if url.is_empty() || url.len() > 65536 {
                    return Ok(());
                }
                let id = self.open_browser(url)?;
                if !subscribe {
                    let index = self.index(id).unwrap();
                    let reply = self.created_reply(index, now)?;
                    self.reply(client, command.tab, reply);
                    return Ok(());
                }
                self.clients.get_mut(&client).unwrap().ready = true;
                self.emit(
                    client,
                    Event {
                        tab: id,
                        kind: EventKind::Created,
                    },
                );
                return self.publish(now);
            }
            CommandKind::Navigate(url) => {
                if url.is_empty() || url.len() > 65536 {
                    return Ok(());
                }
                if let Some(index) = self.index(command.tab) {
                    if !ready || self.tabs[index].url.is_none() {
                        return Ok(());
                    }
                    self.tabs[index].url = Some(url);
                    self.tabs[index].catalog_dirty = true;
                }
                return self.publish(now);
            }
            CommandKind::Open { launch, mode } => {
                if mode == OpenMode::Detached {
                    let reply = self.open_detached(&launch, now)?;
                    self.reply(client, command.tab, reply);
                    return Ok(());
                }
                self.clients.get_mut(&client).unwrap().ready = true;
                let reused = self.restored_launch
                    && !self.tabs.is_empty()
                    && mode == OpenMode::ReuseRestored
                    && launch.arguments.is_empty()
                    && launch.line.is_empty();
                let index = if reused {
                    if self.tabs[0].terminal.is_some() {
                        self.take_control(0, client);
                    }
                    0
                } else {
                    self.open_terminal(&launch, Some(client))?
                };
                self.restored_launch = false;
                self.emit(
                    client,
                    Event {
                        tab: self.tabs[index].id,
                        kind: EventKind::Created,
                    },
                );
                self.publish(now)?;
                if reused {
                    self.send_catalog(client, now)?;
                }
                // The creating window displays its new tab.
                self.show(index, client);
                self.send_frame(index, true, now)?;
                return Ok(());
            }
            _ => {}
        }
        let Some(index) = self.index(command.tab).filter(|_| ready) else {
            return Ok(());
        };
        if let CommandKind::Move { position } = command.kind {
            self.reorder(index, position as usize);
            return self.publish(now);
        }
        if let CommandKind::Attention(severity) = command.kind {
            // Severity only rises until input answers the tab.
            self.set_attention(
                index,
                severity.max(self.tabs[index].attention),
                Some(client),
            );
            return Ok(());
        }
        if self.tabs[index].url.is_some() {
            if matches!(command.kind, CommandKind::Close { .. }) {
                self.close(index);
            } else if matches!(command.kind, CommandKind::Focus { .. }) {
                self.show(index, client);
            }
            return Ok(());
        }
        match command.kind {
            CommandKind::Selection {
                generation,
                operation,
            } => {
                if operation == SelectionOperation::Start {
                    let text = core(
                        self.tabs[index]
                            .terminal
                            .as_mut()
                            .unwrap()
                            .engine
                            .all_text(),
                    )?;
                    self.clients.get_mut(&client).unwrap().selection =
                        Some((command.tab, generation, text, 0));
                }
                let peer = self.clients.get_mut(&client).unwrap();
                let Some((tab, current, text, offset)) = peer.selection.as_mut() else {
                    return Ok(());
                };
                if *tab != command.tab || *current != generation {
                    return Ok(());
                }
                if operation == SelectionOperation::Cancel {
                    peer.selection = None;
                    return Ok(());
                }
                if operation != SelectionOperation::Start && operation != SelectionOperation::Next {
                    return Ok(());
                }
                let end = (*offset + 65536).min(text.len());
                let done = end == text.len();
                let text = text[*offset..end].to_vec();
                *offset = end;
                if done {
                    peer.selection = None;
                }
                self.emit(
                    client,
                    Event {
                        tab: command.tab,
                        kind: EventKind::Selection {
                            generation,
                            done,
                            text,
                        },
                    },
                );
            }
            CommandKind::Control(action) => {
                self.take_control(index, client);
                self.tabs[index].print_client = Some(client);
                let terminal = self.tabs[index].terminal.as_mut().unwrap();
                if action == ControlAction::Break {
                    terminal.pty.send_break()?;
                } else {
                    let effects = core(terminal.engine.control(
                        if action == ControlAction::TogglePrinter {
                            WORMINAL_TOGGLE_PRINTER
                        } else {
                            WORMINAL_PRINT_SCREEN
                        },
                    ))?;
                    let notices = self.tabs[index].effects(effects, now)?;
                    self.notices(command.tab, notices);
                }
            }
            CommandKind::FocusState(focused) => {
                if self.tabs[index].controller == Some(client)
                    && self.tabs[index].mode & MODE_FOCUS != 0
                {
                    {
                        let notices = self.tabs[index].input(
                            if focused { b"\x1b[I" } else { b"\x1b[O" },
                            false,
                            now,
                        )?;
                        self.notices(command.tab, notices);
                    }
                }
            }
            CommandKind::Focus { columns, rows } | CommandKind::Resize { columns, rows } => {
                if matches!(command.kind, CommandKind::Resize { .. })
                    && self.tabs[index].controller != Some(client)
                {
                    return Ok(());
                }
                if columns == 0
                    || columns > WORKSPACE_MAX_COLS
                    || rows == 0
                    || rows > WORKSPACE_MAX_ROWS
                {
                    return Ok(());
                }
                let claim = self.tabs[index].controller != Some(client);
                let revealed = !self.tabs[index].shown;
                self.take_control(index, client);
                self.show(index, client);
                let resized = self.resize(index, columns, rows, now)?;
                if claim || resized || revealed {
                    // A revealed tab resumes from its retained baseline with a delta.
                    self.send_frame(index, claim || resized, now)?;
                }
            }
            CommandKind::Input { bytes, .. } => {
                self.input_operation(command.tab, &bytes, Some(client), now)?;
            }
            CommandKind::Release => {
                if self.tabs[index].controller == Some(client) {
                    self.release(index);
                }
            }
            CommandKind::Close { .. } => self.close(index),
            CommandKind::Scroll { up, amount } => {
                self.take_control(index, client);
                core(
                    self.tabs[index]
                        .terminal
                        .as_mut()
                        .unwrap()
                        .engine
                        .scroll(up, amount as i32),
                )?;
                self.tabs[index].frame_reply = true;
                self.send_frame(index, false, now)?;
            }
            _ => {}
        }
        Ok(())
    }

    pub fn finish_replacement(&mut self, client: ClientId) {
        // Ownership changes only after the complete acknowledgement reached the socket. A congested
        // caller cannot block other tabs, and disconnect before acknowledgement leaves its shell alive.
        if !self
            .clients
            .get(&client)
            .is_some_and(|peer| peer.output.is_empty())
        {
            return;
        }
        let Some((id, url)) = self.pending_replace.remove(&client) else {
            return;
        };
        let Some(index) = self.index(id) else {
            return;
        };
        self.release(index);
        let tab = &mut self.tabs[index];
        tab.terminal.take();
        tab.print_client = None;
        tab.pending_frame = false;
        tab.input_frame = false;
        tab.mode = 0;
        tab.title = b"Browser".to_vec();
        tab.directory.clear();
        tab.url = Some(url);
        tab.catalog_dirty = true;
    }
}
