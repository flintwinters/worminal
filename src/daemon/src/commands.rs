use crate::{
    process,
    service::Service,
    state::{core, ClientId, Tab},
};
use std::io;
use worminal_engine::{MODE_FOCUS, WORMINAL_PRINT_SCREEN, WORMINAL_TOGGLE_PRINTER};
use worminal_protocol::*;
use worminal_workspace::launch::Launch;

fn integer(bytes: &[u8]) -> Option<u32> {
    (bytes.len() == 4).then(|| u32::from_be_bytes(bytes.try_into().unwrap()))
}

fn pair(bytes: &[u8]) -> Option<(u32, u32)> {
    (bytes.len() == 8).then(|| (integer(&bytes[..4]).unwrap(), integer(&bytes[4..]).unwrap()))
}

fn url(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut payload = Payload::new(bytes);
    // Legacy length-delimited strings retain their C-string prefix interpretation.
    let value = payload.string().ok()?.split(|byte| *byte == 0).next()?;
    payload.finish().ok()?;
    (!value.is_empty() && value.len() <= 65536).then(|| value.to_vec())
}

impl Service {
    fn error(&mut self, client: ClientId, message: &[u8]) -> io::Result<()> {
        let mut body = Buffer::default();
        body.put_string(message)?;
        self.queue(client, WIRE_ERROR, 0, body.into_bytes());
        Ok(())
    }

    pub fn message(&mut self, client: ClientId, packet: Packet) -> io::Result<()> {
        let Some(peer) = self.clients.get(&client) else {
            return Ok(());
        };
        let ready = peer.ready;
        let peer_pid = peer.pid;
        let now = self.now();
        match packet.kind {
            WIRE_SERVICE_INFO => {
                self.queue(
                    client,
                    WIRE_SERVICE_INFO,
                    0,
                    std::process::id().to_be_bytes().to_vec(),
                );
                return Ok(());
            }
            WIRE_CAPABILITIES => {
                if !ready && packet.tab == 0 {
                    if let Some(features) = integer(&packet.body) {
                        self.clients.get_mut(&client).unwrap().features =
                            features & WIRE_FEATURE_COPY_ROWS;
                        self.queue(
                            client,
                            WIRE_CAPABILITIES,
                            0,
                            (WIRE_FEATURE_COPY_ROWS | WIRE_FEATURE_SELECT_ALL).to_be_bytes(),
                        );
                    }
                }
                return Ok(());
            }
            WIRE_STOP => {
                self.running = false;
                return Ok(());
            }
            WIRE_HANDIN | WIRE_HANDOFF => {
                return self.handoff_command(client, packet.kind, &packet.body);
            }
            WIRE_BROWSER_REPLACE => {
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
                let Some(url) = url(&packet.body) else {
                    return self.error(
                        client,
                        b"Invalid browser URL: supply one nonempty URL up to 65536 bytes.",
                    );
                };
                if self.pending_replace.contains_key(&client) {
                    return Ok(());
                }
                let id = self.tabs[index].id;
                self.pending_replace.insert(client, (id, url));
                self.queue(client, WIRE_COMMAND_DONE, id, Vec::new());
                self.client_flush(client);
                return Ok(());
            }
            WIRE_ATTACH => {
                self.restored_launch = false;
                self.clients.get_mut(&client).unwrap().ready = true;
                return self.send_catalog(client, now);
            }
            WIRE_BROWSER | WIRE_BROWSER_NAVIGATE => {
                let Some(url) = url(&packet.body) else {
                    return Ok(());
                };
                if packet.kind == WIRE_BROWSER {
                    self.restored_launch = false;
                    let id = self.new_id()?;
                    self.tabs.push(Tab::browser(id, url));
                    self.clients.get_mut(&client).unwrap().ready = true;
                    self.queue(client, WIRE_CREATED, id, Vec::new());
                } else if let Some(index) = self.index(packet.tab) {
                    if !ready || self.tabs[index].url.is_none() {
                        return Ok(());
                    }
                    self.tabs[index].url = Some(url);
                    self.tabs[index].catalog_dirty = true;
                }
                return self.publish(now);
            }
            WIRE_HELLO | WIRE_NEW => {
                let launch = Launch::decode(&packet.body)?;
                self.clients.get_mut(&client).unwrap().ready = true;
                let reused = self.restored_launch
                    && !self.tabs.is_empty()
                    && packet.kind == WIRE_HELLO
                    && launch.arguments.is_empty()
                    && launch.line.is_empty();
                let index = if reused {
                    if self.tabs[0].terminal.is_some() {
                        self.take_control(0, client);
                    }
                    0
                } else {
                    let id = self.new_id()?;
                    self.tabs.push(Tab::shell(
                        id,
                        &launch,
                        false,
                        Some(client),
                        self.reaper.clone(),
                    )?);
                    self.tabs.len() - 1
                };
                self.restored_launch = false;
                self.queue(client, WIRE_CREATED, self.tabs[index].id, Vec::new());
                self.publish(now)?;
                if reused {
                    self.send_catalog(client, now)?;
                }
                self.send_frame(index, true, now)?;
                return Ok(());
            }
            _ => {}
        }
        let Some(index) = self.index(packet.tab).filter(|_| ready) else {
            return Ok(());
        };
        if self.tabs[index].url.is_some() {
            if packet.kind == WIRE_CLOSE {
                self.close(index);
            }
            return Ok(());
        }
        match packet.kind {
            WIRE_SELECT_ALL => {
                let Some((generation, operation)) = pair(&packet.body) else {
                    return Ok(());
                };
                if operation == WIRE_SELECTION_START {
                    let text = core(
                        self.tabs[index]
                            .terminal
                            .as_mut()
                            .unwrap()
                            .engine
                            .all_text(),
                    )?;
                    self.clients.get_mut(&client).unwrap().selection =
                        Some((packet.tab, generation, text, 0));
                }
                let peer = self.clients.get_mut(&client).unwrap();
                let Some((tab, current, text, offset)) = peer.selection.as_mut() else {
                    return Ok(());
                };
                if *tab != packet.tab || *current != generation {
                    return Ok(());
                }
                if operation == WIRE_SELECTION_CANCEL {
                    peer.selection = None;
                    return Ok(());
                }
                if operation != WIRE_SELECTION_START && operation != WIRE_SELECTION_NEXT {
                    return Ok(());
                }
                let end = (*offset + 65536).min(text.len());
                let done = end == text.len();
                let mut body = Buffer::default();
                body.put_u32(generation)?;
                body.put_u32(u32::from(done))?;
                body.put_bytes(&text[*offset..end])?;
                *offset = end;
                if done {
                    peer.selection = None;
                }
                self.queue(client, WIRE_SELECT_ALL, packet.tab, body.into_bytes());
            }
            WIRE_CONTROL => {
                let Some(direction) = integer(&packet.body).filter(|value| *value <= 2) else {
                    return Ok(());
                };
                self.take_control(index, client);
                self.tabs[index].print_client = Some(client);
                let terminal = self.tabs[index].terminal.as_mut().unwrap();
                if direction == 2 {
                    terminal.pty.send_break()?;
                } else {
                    let effects = core(terminal.engine.control(if direction == 0 {
                        WORMINAL_TOGGLE_PRINTER
                    } else {
                        WORMINAL_PRINT_SCREEN
                    }))?;
                    let notices = self.tabs[index].effects(effects, now)?;
                    self.notices(packet.tab, notices);
                }
            }
            WIRE_FOCUS_STATE => {
                if self.tabs[index].controller == Some(client)
                    && self.tabs[index].mode & MODE_FOCUS != 0
                {
                    if let Some(direction) = integer(&packet.body).filter(|value| *value <= 1) {
                        let notices = self.tabs[index].input(
                            if direction == 1 { b"\x1b[I" } else { b"\x1b[O" },
                            false,
                            now,
                        )?;
                        self.notices(packet.tab, notices);
                    }
                }
            }
            WIRE_FOCUS | WIRE_RESIZE => {
                if packet.kind == WIRE_RESIZE && self.tabs[index].controller != Some(client) {
                    return Ok(());
                }
                let Some((columns, rows)) = pair(&packet.body).filter(|&(c, r)| {
                    c > 0 && c <= WORKSPACE_MAX_COLS && r > 0 && r <= WORKSPACE_MAX_ROWS
                }) else {
                    return Ok(());
                };
                let claim = self.tabs[index].controller != Some(client);
                self.take_control(index, client);
                let resized = self.resize(index, columns, rows, now)?;
                if claim || resized {
                    self.send_frame(index, true, now)?;
                }
            }
            WIRE_INPUT => {
                if packet.body.len() > 65536 {
                    return Ok(());
                }
                self.take_control(index, client);
                let notices = self.tabs[index].input(&packet.body, true, now)?;
                self.notices(packet.tab, notices);
                // Unchanged local echo must not consume the first actual PTY response allowance.
                self.send_frame(index, false, now)?;
            }
            WIRE_RELEASE => {
                if self.tabs[index].controller == Some(client) {
                    self.release(index);
                }
            }
            WIRE_CLOSE => self.close(index),
            WIRE_SCROLL => {
                let Some((direction, amount)) =
                    pair(&packet.body).filter(|&(direction, _)| direction <= 1)
                else {
                    return Ok(());
                };
                self.take_control(index, client);
                core(
                    self.tabs[index]
                        .terminal
                        .as_mut()
                        .unwrap()
                        .engine
                        .scroll(direction != 0, amount as i32),
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
