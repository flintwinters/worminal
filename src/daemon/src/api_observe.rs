//! Passive observation and target identity shared by reads, waits and guarded input.
use crate::{
    api::{failure, success},
    service::Service,
    state::{core, Tab},
};
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    io,
    os::unix::fs::MetadataExt,
};
use worminal_protocol::worp::{Reply, Request};

fn fingerprint(value: impl Hash) -> String {
    let mut hash = DefaultHasher::new();
    value.hash(&mut hash);
    format!("{:016x}", hash.finish())
}

pub(crate) fn foreground(tab: &Tab) -> serde_json::Value {
    let Some(group) = tab
        .terminal
        .as_ref()
        .and_then(|t| t.pty.fd())
        .and_then(crate::ssh::foreground_group)
    else {
        return serde_json::Value::Null;
    };
    let root = format!("/proc/{group}");
    let observation = || -> Option<serde_json::Value> {
        let stat = std::fs::read_to_string(format!("{root}/stat")).ok()?;
        let start = stat
            .rsplit_once(')')?
            .1
            .split_whitespace()
            .nth(19)?
            .parse::<u64>()
            .ok()?;
        let executable = std::fs::read_link(format!("{root}/exe")).ok()?;
        let metadata = std::fs::metadata(format!("{root}/exe")).ok()?;
        let argv = std::fs::read(format!("{root}/cmdline")).ok()?;
        Some(
            serde_json::json!({"process_group":group, "start_time":start,
            "executable":executable.to_string_lossy(),
            "identity":fingerprint((group, start, metadata.dev(), metadata.ino(), argv))}),
        )
    };
    observation().unwrap_or(serde_json::Value::Null)
}

pub(crate) fn state(tab: &Tab, workspace: &str) -> serde_json::Value {
    let foreground = foreground(tab);
    let guard = foreground["identity"]
        .as_str()
        .map(|identity| format!("{workspace}:{}:{identity}", tab.id));
    serde_json::json!({"busy":tab.mode & worminal_engine::MODE_BUSY != 0,
        "controlled":tab.controller.is_some(), "shown":tab.shown, "foreground":foreground,
        "input_guard":guard, "shell":{"phase":tab.shell.phase, "command_id":tab.shell.command_id,
        "exit_status":tab.shell.exit_status}})
}

impl Service {
    pub(crate) fn observe(&mut self, tab: u32, tail: u32, now: i64) -> io::Result<Reply> {
        let Some(index) = self.index(tab) else {
            return Ok(failure("Tab not found; run worminald api list."));
        };
        let tab = &mut self.tabs[index];
        if tab.terminal.is_none() {
            return Ok(failure("This operation requires a terminal tab."));
        }
        tab.refresh_directory(now);
        let mut value: serde_json::Value = serde_json::from_str(&super::api::read(tab, now)?)?;
        value["state"] = state(tab, &self.workspace_id);
        value["directory"] = serde_json::json!(String::from_utf8_lossy(&tab.directory));
        value["title"] = serde_json::json!(String::from_utf8_lossy(&tab.title));
        // Include output generation so output that only enters history still wakes a waiter.
        value["revision"] = serde_json::json!(format!(
            "{}:{}",
            self.workspace_id,
            fingerprint((value.to_string(), tab.output_revision))
        ));
        value["workspace"] = serde_json::json!(self.workspace_id);
        if tail != 0 {
            let (bytes, total) = core(tab.terminal.as_mut().unwrap().engine.tail(tail as i32))?;
            let text = String::from_utf8_lossy(&bytes);
            let lines: Vec<_> = text
                .strip_suffix('\n')
                .unwrap_or(&text)
                .split('\n')
                .collect();
            value["lines"] = serde_json::json!(lines);
            value["total_lines"] = serde_json::json!(total);
            value["truncated"] = serde_json::json!(total > tail as i32);
            value["text_source"] = serde_json::json!("tail");
        } else {
            value["text_source"] = serde_json::json!("viewport");
        }
        Ok(success(value.to_string()))
    }

    pub(crate) fn extended(&mut self, request: &Request, now: i64) -> io::Result<Option<Reply>> {
        let (tab, guard, bytes) = match request {
            Request::ReadMany { tabs, tail } => {
                let mut results = Vec::new();
                let mut budget = crate::state::CLIENT_QUEUE - 4096;
                for tab in tabs {
                    let reply = self.observe(*tab, *tail, now)?;
                    let mut value: serde_json::Value = serde_json::from_str(&reply.json)?;
                    value["id"] = serde_json::json!(tab);
                    let size = value.to_string().len() + 1;
                    if size > budget {
                        value = serde_json::json!({"ok":false,"id":tab,"error":"Batch reply budget exceeded; read fewer tabs or rows."});
                    } else {
                        budget -= size;
                    }
                    results.push(value);
                }
                let ok = results.iter().all(|r| r["ok"] == true);
                return Ok(Some(Reply {
                    success: ok,
                    json: serde_json::json!({"ok":ok,"results":results}).to_string(),
                }));
            }
            Request::Keys { tab, names, guard } => {
                let Some(index) = self.index(*tab) else {
                    return Ok(Some(failure("Tab not found.")));
                };
                let mut bytes = Vec::new();
                for name in names {
                    match worminal_engine::keys::named(name, self.tabs[index].mode) {
                        Ok(key) => bytes.extend(key),
                        Err(message) => return Ok(Some(failure(message))),
                    }
                }
                (*tab, guard, bytes)
            }
            Request::Paste { tab, text, guard } => {
                let Some(index) = self.index(*tab) else {
                    return Ok(Some(failure("Tab not found.")));
                };
                // Multiline input needs bracketed paste: ordinary LF would submit commands.
                let bracketed = self.tabs[index].mode & worminal_engine::MODE_BRCKTPASTE != 0;
                if !bracketed && text.contains(['\n', '\r']) {
                    return Ok(Some(failure("Multiline stdin requires bracketed paste mode; use explicit key Enter to submit.")));
                }
                let mut bytes = text.as_bytes().to_vec();
                let length = worminal_engine::PasteFilter::new(bracketed).filter(&mut bytes);
                bytes.truncate(length);
                if bracketed {
                    bytes.splice(0..0, worminal_engine::paste_begin().iter().copied());
                    bytes.extend_from_slice(worminal_engine::paste_end());
                }
                (*tab, guard, bytes)
            }
            Request::GuardedInput { tab, bytes, guard } => (*tab, guard, bytes.clone()),
            _ => return Ok(None),
        };
        if !guard.is_empty() {
            let matches = self.index(tab).is_some_and(|i| {
                state(&self.tabs[i], &self.workspace_id)["input_guard"].as_str() == Some(guard)
            });
            if !matches {
                return Ok(Some(failure(
                    "Input guard no longer matches the workspace/foreground process; read again.",
                )));
            }
        }
        Ok(Some(self.input_operation(tab, &bytes, None, now)?))
    }
}
