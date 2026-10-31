//! One bounded byte queue for client output, PTY input, and independent bridge directions.
use std::io::{self, Write};

pub struct Queue {
    bytes: Vec<u8>,
    sent: usize,
    limit: usize,
}

impl Queue {
    pub fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            sent: 0,
            limit,
        }
    }
    pub fn pending(&self) -> &[u8] {
        &self.bytes[self.sent..]
    }
    pub fn len(&self) -> usize {
        self.bytes.len() - self.sent
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
        if bytes.len() > self.limit - self.len() {
            return Err(io::Error::new(
                io::ErrorKind::OutOfMemory,
                "bounded transport queue is full",
            ));
        }
        if self.sent != 0 {
            self.bytes.drain(..self.sent);
            self.sent = 0;
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    /// A readiness callback spends at most its byte budget. A blocked direction cannot stall another.
    pub fn flush(&mut self, output: &mut impl Write, mut budget: usize) -> io::Result<usize> {
        let mut total = 0;
        while budget != 0 && !self.is_empty() {
            let bytes = &self.pending()[..budget.min(self.len())];
            match output.write(bytes) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "transport closed during output",
                    ))
                }
                Ok(count) => {
                    self.sent += count;
                    total += count;
                    budget -= count;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error),
            }
        }
        if self.sent == self.bytes.len() {
            self.sent = 0;
            self.bytes.clear();
        }
        Ok(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Slow {
        bytes: Vec<u8>,
        block: bool,
    }
    impl Write for Slow {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.block {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            self.bytes.extend_from_slice(&bytes[..bytes.len().min(2)]);
            self.block = true;
            Ok(bytes.len().min(2))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn partial_writes_bounds_and_fair_budgets_preserve_byte_order() {
        let mut queue = Queue::new(5);
        queue.append(b"abcde").unwrap();
        let mut output = Slow {
            bytes: Vec::new(),
            block: false,
        };
        assert_eq!(queue.flush(&mut output, 4).unwrap(), 2);
        assert!(queue.append(b"XYZ").is_err());
        assert_eq!(queue.pending(), b"cde");
        queue.append(b"fg").unwrap();
        output.block = false;
        assert_eq!(queue.flush(&mut output, 1).unwrap(), 1);
        assert_eq!(queue.pending(), b"defg");
        while !queue.is_empty() {
            output.block = false;
            queue.flush(&mut output, 4).unwrap();
        }
        assert_eq!(output.bytes, b"abcdefg");
    }
}
