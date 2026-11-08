use crate::{service::Service, state::core};
use std::io::{self, Read};
use worminal_protocol::{WORKSPACE_IO_BYTES, WORKSPACE_RESYNC_BACKLOG_BYTES};

impl Service {
    pub fn client_flush(&mut self, id: u64) {
        let failed = self.clients.get_mut(&id).is_some_and(|client| {
            client
                .output
                .flush(&mut client.socket, WORKSPACE_IO_BYTES as usize)
                .is_err()
        });
        if failed {
            self.drop_client(id);
        }
        self.finish_replacement(id);
    }

    pub fn client_ready(&mut self, id: u64, read: bool, write: bool) -> io::Result<()> {
        if write {
            self.client_flush(id);
        }
        if !read {
            return Ok(());
        }
        let Some(client) = self.clients.get_mut(&id) else {
            return Ok(());
        };
        let mut bytes = [0u8; 4096];
        let count = match client.socket.read(&mut bytes) {
            Ok(0) => {
                self.drop_client(id);
                return Ok(());
            }
            Ok(count) => count,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                return Ok(())
            }
            Err(_) => {
                self.drop_client(id);
                return Ok(());
            }
        };
        let mut bytes = &bytes[..count];
        // One 4 KiB read bounds work even if a client sends an uninterrupted stream of empty packets.
        while !bytes.is_empty() {
            let Some(client) = self.clients.get_mut(&id) else {
                break;
            };
            let (used, packet) = match client.decoder.push(bytes) {
                Ok(decoded) => decoded,
                Err(_) => {
                    self.drop_client(id);
                    break;
                }
            };
            bytes = &bytes[used..];
            if let Some(packet) = packet {
                if let Err(error) = self.message(id, packet) {
                    if matches!(
                        error.kind(),
                        io::ErrorKind::InvalidData | io::ErrorKind::InvalidInput
                    ) {
                        self.drop_client(id);
                        break;
                    }
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    pub fn terminal_ready(&mut self, id: u32, read: bool, write: bool) -> io::Result<()> {
        let Some(index) = self.index(id) else {
            return Ok(());
        };
        let Some(terminal) = &mut self.tabs[index].terminal else {
            return Ok(());
        };
        if write {
            if let Some(master) = terminal.pty.master() {
                match terminal.input.flush(master, WORKSPACE_IO_BYTES as usize) {
                    Ok(count) if count != 0 => self.tabs[index].input_frame = true,
                    Ok(_) => {}
                    Err(_) => {
                        terminal.pty.stop();
                        return Ok(());
                    }
                }
            }
        }
        if !read {
            return Ok(());
        }
        let terminal = self.tabs[index].terminal.as_mut().unwrap();
        let Some(master) = terminal.pty.master() else {
            return Ok(());
        };
        let mut bytes = [0u8; worminal_engine::WORMINAL_PENDING_BYTES as usize];
        let Some(capacity) = bytes.len().checked_sub(terminal.pending.len()) else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "oversized native pending output",
            ));
        };
        if capacity == 0 {
            terminal.pty.stop();
            return Ok(());
        }
        let count = match master.read(&mut bytes[..capacity]) {
            Ok(0) => {
                terminal.pty.stop();
                return Ok(());
            }
            Ok(count) => count,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                return Ok(())
            }
            Err(_) => {
                terminal.pty.stop();
                return Ok(());
            }
        };
        let now = self.now();
        let notices = self.tabs[index].feed(&bytes[..count], now)?;
        self.notices(id, notices);
        self.tabs[index].refresh_directory_after_output(now);
        if (self.tabs[index].input_frame
            || (self.tabs[index].controller.is_some() && self.tabs[index].screen.bytes.is_empty()))
            && self.send_frame(index, false, now)?
        {
            self.tabs[index].input_frame = false;
        }
        Ok(())
    }

    pub fn resync(&mut self, now: i64) -> io::Result<()> {
        let clients: Vec<_> = self
            .clients
            .values_mut()
            .filter_map(|client| {
                if client.needs_resync
                    && client.output.len() < WORKSPACE_RESYNC_BACKLOG_BYTES as usize
                {
                    client.needs_resync = false;
                    Some(client.id)
                } else {
                    None
                }
            })
            .collect();
        for client in clients {
            for index in 0..self.tabs.len() {
                if self.tabs[index].controller == Some(client) {
                    // No queued bytes were discarded: resume from the queued baseline, not a full snapshot.
                    self.send_frame(index, false, now)?;
                }
            }
        }
        Ok(())
    }

    pub fn resize(&mut self, index: usize, columns: u32, rows: u32, now: i64) -> io::Result<bool> {
        let terminal = self.tabs[index].terminal.as_mut().unwrap();
        let frame = core(terminal.engine.frame(now))?;
        let resized = frame.columns != columns as i32 || frame.rows != rows as i32;
        if resized {
            core(terminal.engine.resize(columns as i32, rows as i32))?;
            terminal.pty.resize(columns as u16, rows as u16)?;
        }
        Ok(resized)
    }
}
