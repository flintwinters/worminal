//! Deferred unary replies: wait without controlling a terminal or blocking the service loop.
use crate::{api::failure_code, service::Service, state::ClientId};
use std::io;
use worminal_protocol::worp::Reply;
const INTERVAL: i64 = 50_000_000;
pub(crate) enum Condition {
    Revision(String),
    Command { workspace: String, id: u64 },
}
pub(crate) struct Pending {
    pub tab: u32,
    pub condition: Condition,
    pub deadline: i64,
}
fn outcome(mut reply: Reply, condition: &Condition, timed_out: bool) -> io::Result<Option<Reply>> {
    if !reply.success {
        return Ok(Some(reply));
    }
    let mut value: serde_json::Value = serde_json::from_str(&reply.json)?;
    let ready = match condition {
        Condition::Revision(after) => {
            let changed = value["revision"].as_str() != Some(after);
            value["changed"] = serde_json::json!(changed);
            changed
        }
        Condition::Command { workspace, id } => {
            if value["workspace"].as_str() != Some(workspace) {
                return Ok(Some(failure_code(
                    "workspace_changed",
                    "Workspace changed since command observation.",
                    Some("Read the tab again."),
                )));
            }
            if value["state"]["shell"]["phase"].is_null() {
                return Ok(Some(failure_code(
                    "shell_integration_unavailable",
                    "No shell markers observed.",
                    Some("See api help shell-bash or shell-zsh."),
                )));
            }
            let completed = value["completed"].as_bool().unwrap_or(false);
            let current = value["state"]["shell"]["command_id"].as_u64().unwrap_or(0);
            if !completed && current > *id {
                return Ok(Some(failure_code(
                    "command_unavailable",
                    "Command completion is no longer retained.",
                    Some("Read the current shell state."),
                )));
            }
            value["command_id"] = serde_json::json!(id);
            value["completed"] = serde_json::json!(completed);
            if !completed {
                value["exit_status"] = serde_json::Value::Null;
            }
            completed
        }
    };
    if !ready && !timed_out {
        return Ok(None);
    }
    value["timed_out"] = serde_json::json!(!ready && timed_out);
    reply.json = value.to_string();
    Ok(Some(reply))
}
impl Service {
    fn wait_observation(&mut self, tab: u32, condition: &Condition, now: i64) -> io::Result<Reply> {
        let mut reply = self.observe(tab, 0, now)?;
        if let Condition::Command { id, .. } = condition {
            if reply.success {
                let mut value: serde_json::Value = serde_json::from_str(&reply.json)?;
                let command = self
                    .index(tab)
                    .and_then(|i| self.tabs[i].output.completed(*id));
                value["completed"] = serde_json::json!(command.is_some());
                value["exit_status"] = serde_json::json!(command.and_then(|c| c.status));
                reply.json = value.to_string();
            }
        }
        Ok(reply)
    }
    pub(crate) fn start_api_wait(
        &mut self,
        client: ClientId,
        tab: u32,
        condition: Condition,
        timeout_ms: u32,
        now: i64,
    ) -> io::Result<()> {
        if self.api_waits.contains_key(&client) || self.api_waits.len() >= 32 {
            self.reply(
                client,
                tab,
                failure_code(
                    "wait_limit",
                    "At most one pending wait per client and 32 per workspace.",
                    None,
                ),
            );
            return Ok(());
        }
        if let Some(reply) = outcome(
            self.wait_observation(tab, &condition, now)?,
            &condition,
            timeout_ms == 0,
        )? {
            self.reply(client, tab, reply);
        } else {
            self.api_waits.insert(
                client,
                Pending {
                    tab,
                    condition,
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
                self.wait_observation(wait.tab, &wait.condition, now)?,
                &wait.condition,
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
