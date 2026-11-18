//! Ordered service-info acknowledgements used to fence GUI replacement snapshots.
use std::collections::VecDeque;

#[derive(Default)]
pub(super) struct Fences {
    requested: u64,
    completed: u64,
    pending: VecDeque<u64>,
}

impl Fences {
    pub(super) fn request(&mut self) -> u64 {
        self.requested = self
            .requested
            .checked_add(1)
            .expect("update fence counter exhausted");
        self.pending.push_back(self.requested);
        self.requested
    }

    pub(super) fn acknowledge(&mut self) {
        if let Some(revision) = self.pending.pop_front() {
            self.completed = revision;
        }
    }

    pub(super) fn complete(&self, revision: u64) -> bool {
        self.completed >= revision
    }

    pub(super) fn disconnected(&mut self) {
        self.pending.clear();
    }

    pub(super) fn reconnect(&mut self) -> bool {
        if self.completed == self.requested {
            return false;
        }
        // The old stream cannot reply again. One new reply covers all outstanding fences;
        // it follows the new stream's catalog. Never replay terminal input here.
        self.pending.push_back(self.requested);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::Fences;

    #[test]
    fn reconnect_reissues_the_lost_fence_without_inventing_an_acknowledgement() {
        let mut fences = Fences::default();
        assert!(!fences.reconnect());
        let first = fences.request();
        let latest = fences.request();
        fences.disconnected();
        assert!(!fences.complete(first));
        assert!(!fences.complete(latest));
        assert!(fences.reconnect());
        assert!(!fences.complete(latest));
        fences.acknowledge();
        assert!(fences.complete(first));
        assert!(fences.complete(latest));
        assert!(!fences.reconnect());
    }

    #[test]
    fn a_late_reply_cannot_complete_a_newer_fence() {
        let mut fences = Fences::default();
        let first = fences.request();
        let latest = fences.request();
        fences.acknowledge();
        assert!(fences.complete(first));
        assert!(!fences.complete(latest));
        fences.acknowledge();
        assert!(fences.complete(latest));
    }
}
