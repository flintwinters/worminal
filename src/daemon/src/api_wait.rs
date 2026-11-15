//! Deferred unary replies: wait without controlling a terminal or blocking the service loop.
use crate::{api::failure, service::Service, state::ClientId};
use std::io;
use worminal_protocol::worp::Reply;
const INTERVAL: i64 = 50_000_000;
pub(crate) struct Pending {
    pub tab: u32,
    pub after: String,
    pub deadline: i64,
}
fn outcome(mut reply: Reply, after: &str, timed_out: bool) -> io::Result<Option<Reply>> {
    if !reply.success {
        return Ok(Some(reply));
    }
    let mut value: serde_json::Value = serde_json::from_str(&reply.json)?;
    let changed = value["revision"].as_str() != Some(after);
    if !changed && !timed_out {
        return Ok(None);
    }
    value["changed"] = serde_json::json!(changed);
    value["timed_out"] = serde_json::json!(!changed && timed_out);
    reply.json = value.to_string();
    Ok(Some(reply))
}
impl Service {
    pub(crate) fn start_api_wait(
        &mut self,
        client: ClientId,
        tab: u32,
        after: String,
        timeout_ms: u32,
        now: i64,
    ) -> io::Result<()> {
        if self.api_waits.contains_key(&client) || self.api_waits.len() >= 32 {
            self.reply(
                client,
                tab,
                failure("At most one pending wait per client and 32 per workspace."),
            );
            return Ok(());
        }
        if let Some(reply) = outcome(self.observe(tab, 0, now)?, &after, timeout_ms == 0)? {
            self.reply(client, tab, reply);
        } else {
            self.api_waits.insert(
                client,
                Pending {
                    tab,
                    after,
                    deadline: now + i64::from(timeout_ms) * 1_000_000,
                },
            );
            // A new caller must not postpone an older wait's next observation.
            self.api_wait_check = self.api_wait_check.min(now + INTERVAL);
        }
        Ok(())
    }
    pub(crate) fn finish_api_waits(&mut self, now: i64) -> io::Result<()> {
        let due = self.api_waits.values().any(|w| w.deadline <= now);
        if now < self.api_wait_check && !due {
            return Ok(());
        }
        let waits = std::mem::take(&mut self.api_waits);
        for (client, wait) in waits {
            if !self.clients.contains_key(&client) {
                continue;
            }
            if let Some(reply) = outcome(
                self.observe(wait.tab, 0, now)?,
                &wait.after,
                now >= wait.deadline,
            )? {
                self.reply(client, wait.tab, reply);
            } else {
                self.api_waits.insert(client, wait);
            }
        }
        self.api_wait_check = now + INTERVAL;
        Ok(())
    }
}
