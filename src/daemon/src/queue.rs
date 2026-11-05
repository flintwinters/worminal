//! One bounded byte queue for client output, PTY input, and independent bridge directions.
use std::io::{self, Write};

pub struct Queue {
    bytes: Vec<u8>,
    sent: usize,
    limit: usize,
}

impl Queue {
    pub fn from_bytes(bytes: Vec<u8>, limit: usize) -> io::Result<Self> {
        if bytes.len() > limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "imported queue exceeds its bound",
            ));
        }
        Ok(Self {
            bytes,
            sent: 0,
            limit,
        })
    }
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
    pub fn remaining(&self) -> usize {
        self.limit - self.len()
    }

    fn check_append(&self, count: usize) -> io::Result<()> {
        if count > self.remaining() {
            return Err(io::Error::new(
                io::ErrorKind::OutOfMemory,
                "bounded transport queue is full",
            ));
        }
        Ok(())
    }

    /// Transfer a prepared complete frame directly when no older bytes remain queued.
    pub fn append_owned(&mut self, bytes: Vec<u8>) -> io::Result<()> {
        if self.is_empty() {
            self.check_append(bytes.len())?;
            self.bytes = bytes;
            self.sent = 0;
            Ok(())
        } else {
            self.append(&bytes)
        }
    }

    pub fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.check_append(bytes.len())?;
        // Amortize compaction: moving the live suffix after every short write
        // makes congested streams copy their backlog on every append. Until
        // half is consumed, use/grow its tail within the existing storage bound.
        if self.sent != 0
            && (self.sent >= self.len() || bytes.len() > self.limit - self.bytes.len())
        {
            self.bytes.drain(..self.sent);
            self.sent = 0;
        }
        let required = self.bytes.len() + bytes.len();
        if required > self.bytes.capacity() && self.bytes.capacity().saturating_mul(2) > self.limit {
            self.bytes.reserve_exact(bytes.len());
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

    #[test]
    fn short_writes_do_not_repeatedly_move_the_live_backlog() {
        let mut queue = Queue::new(16);
        queue.append(b"abcdefgh").unwrap();
        queue.flush(&mut std::io::sink(), 2).unwrap();
        queue.append(b"ij").unwrap();
        assert_eq!(queue.pending(), b"cdefghij");
        assert_eq!(queue.sent, 2);
        assert_eq!(queue.remaining(), 8);
        let before = queue.bytes.clone();
        assert!(queue.append(&[0; 9]).is_err());
        assert_eq!(queue.bytes, before);
        queue.flush(&mut std::io::sink(), 6).unwrap();
        queue.append(b"klmnop").unwrap();
        assert_eq!(queue.pending(), b"ijklmnop");
        assert_eq!(queue.sent, 0);
        queue.flush(&mut std::io::sink(), usize::MAX).unwrap();
        assert!(queue.bytes.is_empty());
        assert_eq!(queue.sent, 0);
    }

    #[test]
    fn full_queues_compact_before_exceeding_the_storage_bound() {
        let mut queue = Queue::new(8);
        queue.append(b"abcdefgh").unwrap();
        for _ in 0..100 {
            queue.flush(&mut std::io::sink(), 2).unwrap();
            queue.append(b"ij").unwrap();
            assert_eq!(queue.len(), 8);
            assert!(queue.bytes.len() <= 8);
            assert!(queue.bytes.capacity() <= 8);
        }
    }

    #[test]
    fn owned_frames_transfer_storage_and_preserve_queued_bytes_on_rejection() {
        let mut queue = Queue::new(8);
        let bytes = b"abcdefgh".to_vec();
        let storage = bytes.as_ptr();
        queue.append_owned(bytes).unwrap();
        assert_eq!(queue.pending().as_ptr(), storage);
        assert!(queue.append_owned(b"x".to_vec()).is_err());
        assert_eq!(queue.pending(), b"abcdefgh");
        queue.flush(&mut std::io::sink(), 6).unwrap();
        queue.append_owned(b"ijklmn".to_vec()).unwrap();
        assert_eq!(queue.pending(), b"ghijklmn");
        queue.flush(&mut std::io::sink(), usize::MAX).unwrap();
        assert!(queue.append_owned(vec![0; 9]).is_err());
        assert!(queue.is_empty());
        queue.append_owned(b"next".to_vec()).unwrap();
        assert_eq!(queue.pending(), b"next");
    }

}
