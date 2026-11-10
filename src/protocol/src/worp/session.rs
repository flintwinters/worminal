//! Ordered Worp command/event delivery; Unix sockets, SSH pipes and TCP share this binding.
//! Sessions never retry commands. The caller owns authentication, deadlines and subscriptions.
use super::{commands::Command, events::Event, stream::read_packet, v2_commands, v2_events};
use std::io::{self, Read, Write};

pub struct Session<R, W> {
    reader: R,
    writer: W,
}
impl<R: Read, W: Write> Session<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Self { reader, writer }
    }
    pub fn send(&mut self, command: &Command) -> io::Result<()> {
        self.writer
            .write_all(&v2_commands::encode(command)?.encode()?)?;
        self.writer.flush()
    }
    /// Return the next understood event. Unknown v2 extensions are skipped, preserving ordering.
    pub fn receive(&mut self) -> io::Result<Event> {
        loop {
            if let Some(event) = v2_events::decode(read_packet(&mut self.reader)?)? {
                return Ok(event);
            }
        }
    }
    pub fn into_parts(self) -> (R, W) {
        (self.reader, self.writer)
    }
}
