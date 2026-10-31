use crate::{handoff, handoff_io::Listener, process, service::Service, upgrade_io::ReceiverChild};
use std::os::fd::AsFd;

impl Service {
    pub fn try_upgrade(&mut self) -> bool {
        let Some(watch) = &mut self.upgrade_watch else {
            return false;
        };
        let replacement = match watch.replacement(self.pending_replace.is_empty()) {
            Ok(Some(replacement)) => replacement,
            Ok(None) => return false,
            Err(error) => {
                process::diagnostic(&format!("Automatic upgrade watch disabled: {error}"));
                self.upgrade_watch = None;
                return false;
            }
        };
        let token = format!("upgrade-{}-{}", std::process::id(), replacement.inode);
        let result = (|| {
            let receiving = Listener::bind(token.as_bytes())?;
            let child =
                ReceiverChild::spawn(&replacement.path, self.listener.as_fd(), receiving.as_fd())?;
            match handoff::send(
                token.as_bytes(),
                &mut self.tabs,
                &self.reaper,
                Some(self.next_id),
            ) {
                Ok(commit) => {
                    process::diagnostic(&format!("Daemon upgrade committed to pid {}", child.pid));
                    child.committed(&commit);
                    Ok(true)
                }
                Err(error) => {
                    if let Err(rejection) = child.reject() {
                        process::error(&format!("Could not retire rejected receiver: {rejection}"));
                    }
                    Err(error)
                }
            }
        })();
        match result {
            Ok(committed) => committed,
            Err(error) => {
                process::error(&format!(
                    "Daemon upgrade rejected; live tabs retained: {error}"
                ));
                false
            }
        }
    }
}
