use crate::{handoff, handoff_io::Listener, process, service::Service, state::ClientId};
use std::io;
use worminal_protocol::{Buffer, Payload, WIRE_COMMAND_DONE, WIRE_ERROR, WIRE_HANDIN};

impl Service {
    pub fn handoff_command(&mut self, client: ClientId, kind: u32, bytes: &[u8]) -> io::Result<()> {
        let mut operation = || -> io::Result<()> {
            let mut payload = Payload::new(bytes);
            let name = payload.string()?.split(|byte| *byte == 0).next().unwrap();
            payload.finish()?;
            if !self.pending_replace.is_empty() {
                return Err(io::Error::other(
                    "browser conversion acknowledgement is still pending",
                ));
            }
            if kind == WIRE_HANDIN {
                if self.handin.is_some() {
                    return Err(io::Error::other("handoff listener already exists"));
                }
                let id = self.next_handin;
                self.next_handin = id
                    .checked_add(1)
                    .ok_or_else(|| io::Error::other("handoff IDs exhausted"))?;
                self.handin = Some((id, Listener::bind(name)?));
            } else {
                let last = self.tabs.last().map_or(0, |tab| tab.id);
                handoff::send(name, &mut self.tabs, &self.reaper, None)?;
                self.closed = last;
                self.fallback = 0;
            }
            Ok(())
        };
        let result = operation();
        let mut reply = Buffer::default();
        if let Err(error) = &result {
            process::diagnostic(&format!("Handoff rejected before commit: {error}"));
        }
        reply.put_string(if result.is_ok() {
            b""
        } else {
            b"Transfer failed before commit; donor sessions retained."
        })?;
        self.queue(
            client,
            if result.is_ok() {
                WIRE_COMMAND_DONE
            } else {
                WIRE_ERROR
            },
            0,
            reply.into_bytes(),
        );
        Ok(())
    }

    pub fn receive_handoff(&mut self, id: u64, signals: &crate::reactor::Signals) {
        let Some((current, listener)) = &self.handin else {
            return;
        };
        if *current != id {
            return;
        }
        let result = listener.accept().and_then(|channel| {
            handoff::receive(
                channel,
                &mut self.tabs,
                &mut self.next_id,
                &self.reaper,
                handoff::Incoming::Append,
                signals,
            )
        });
        match result {
            Ok(()) => {
                self.handin = None;
            }
            Err(error) => process::diagnostic(&format!(
                "Handoff import rejected; donor retains ownership: {error}"
            )),
        }
    }
}
