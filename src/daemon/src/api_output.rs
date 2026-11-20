//! Bounded command observations, backed by engine row anchors rather than another output log.
use crate::{
    api::{failure_code, success},
    service::Service,
    state::{core, ShellState},
};
use std::{collections::VecDeque, io};
use worminal_engine::{Engine, OutputPosition};
use worminal_protocol::worp::Reply;

pub(crate) struct Command {
    pub id: u64,
    pub status: Option<i32>,
    bytes: Vec<u8>,
    gap: bool,
    truncated: bool,
}
#[derive(Default)]
pub(crate) struct History {
    active: Option<(u64, OutputPosition)>,
    commands: VecDeque<Command>,
}
impl History {
    pub(crate) fn completed(&self, id: u64) -> Option<&Command> {
        self.commands.iter().find(|c| c.id == id)
    }
    pub(crate) fn event(
        &mut self,
        marker: u8,
        shell: &ShellState,
        engine: &mut Engine,
    ) -> io::Result<()> {
        match marker {
            b'C' => self.active = Some((shell.command_id, core(engine.output_position())?)),
            b'D' => {
                if let Some((id, start)) = self.active.take() {
                    let end = core(engine.output_position())?;
                    let (mut bytes, _, flags) = core(engine.output(start, end))?;
                    // Retained memory follows the byte budget, not the temporary extraction capacity.
                    bytes.shrink_to_fit();
                    self.commands.push_back(Command {
                        id,
                        status: shell.exit_status,
                        bytes,
                        gap: flags & 1 != 0,
                        truncated: flags & 2 != 0,
                    });
                    while self.commands.len() > 16
                        || self.commands.iter().map(|c| c.bytes.len()).sum::<usize>() > 256 * 1024
                    {
                        self.commands.pop_front();
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}

pub(crate) fn cursor(workspace: &str, tab: u32, position: OutputPosition, revision: u64) -> String {
    format!(
        "{workspace}:{tab}:{}:{}:{}:{revision}",
        position.epoch, position.row, position.column
    )
}
fn decode(token: &str, workspace: &str, tab: u32) -> Option<(OutputPosition, u64)> {
    let fields: Vec<_> = token.split(':').collect();
    if fields.len() != 6 || fields[0] != workspace || fields[1].parse::<u32>().ok()? != tab {
        return None;
    }
    Some((
        OutputPosition {
            epoch: fields[2].parse().ok()?,
            row: fields[3].parse().ok()?,
            column: fields[4].parse().ok()?,
        },
        fields[5].parse().ok()?,
    ))
}
pub(crate) fn stamp(value: &serde_json::Value) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    value["revision"].as_str().hash(&mut hash);
    hash.finish()
}
fn lines(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    if text.is_empty() {
        Vec::new()
    } else {
        text.strip_suffix('\n')
            .unwrap_or(&text)
            .split('\n')
            .map(str::to_owned)
            .collect()
    }
}
fn command_observation(
    tab: &mut crate::state::Tab,
    command_id: u64,
    value: &mut serde_json::Value,
) -> io::Result<bool> {
    if let Some(command) = tab.output.completed(command_id) {
        value["lines"] = serde_json::json!(lines(&command.bytes));
        value["gap"] = serde_json::json!(command.gap);
        value["truncated"] = serde_json::json!(command.truncated);
        value["completed"] = serde_json::json!(true);
        value["exit_status"] = serde_json::json!(command.status);
    } else if let Some((active, start)) = tab
        .output
        .active
        .filter(|(active, _)| *active == command_id)
    {
        let engine = &mut tab.terminal.as_mut().unwrap().engine;
        let end = core(engine.output_position())?;
        let (bytes, _, flags) = core(engine.output(start, end))?;
        value["lines"] = serde_json::json!(lines(&bytes));
        value["gap"] = serde_json::json!(flags & 1 != 0);
        value["truncated"] = serde_json::json!(flags & 2 != 0);
        value["completed"] = serde_json::json!(false);
        value["exit_status"] = serde_json::Value::Null;
        debug_assert_eq!(active, command_id);
    } else {
        return Ok(false);
    }
    value["command_id"] = serde_json::json!(command_id);
    value["text_source"] = serde_json::json!("command");
    Ok(true)
}
impl Service {
    pub(crate) fn read_output(
        &mut self,
        id: u32,
        since: &str,
        command_id: u64,
        now: i64,
    ) -> io::Result<Reply> {
        let mut reply = self.observe(id, 0, now)?;
        if !reply.success {
            return Ok(reply);
        }
        let mut value: serde_json::Value = serde_json::from_str(&reply.json)?;
        let current_stamp = stamp(&value);
        let index = self.index(id).unwrap();
        let tab = &mut self.tabs[index];
        if command_id != 0 {
            if !command_observation(tab, command_id, &mut value)? {
                return Ok(failure_code(
                    "command_unavailable",
                    "Command is unknown or no longer retained.",
                    Some("Read current shell state; command observations reset on handoff."),
                ));
            }
        } else {
            let engine = &mut tab.terminal.as_mut().unwrap().engine;
            let end = core(engine.output_position())?;
            let decoded = decode(since, &self.workspace_id, id);
            let (mut start, revision) = decoded.unwrap_or((
                OutputPosition {
                    epoch: u64::MAX,
                    row: 0,
                    column: 0,
                },
                u64::MAX,
            ));
            // The last physical row is mutable: replace it in the consumer, do not append it.
            start.column = 0;
            let (bytes, next, flags) =
                if revision == current_stamp && decoded.is_some() && start.epoch == end.epoch {
                    (Vec::new(), end, 0)
                } else {
                    let mut through = end;
                    through.column = value["columns"].as_i64().unwrap_or(0) as i32;
                    let (bytes, next, flags) = core(engine.output(start, through))?;
                    (bytes, if flags & 2 == 0 { end } else { next }, flags)
                };
            value["lines"] = serde_json::json!(lines(&bytes));
            value["gap"] = serde_json::json!(flags & 1 != 0 || decoded.is_none());
            value["truncated"] = serde_json::json!(flags & 2 != 0);
            // A paginated cursor must not claim all output from this generation was consumed.
            value["output_cursor"] = serde_json::json!(cursor(
                &self.workspace_id,
                id,
                next,
                if flags & 2 != 0 {
                    u64::MAX
                } else {
                    current_stamp
                }
            ));
            value["replace_last_line"] = serde_json::json!(
                decoded.is_some_and(|(_, revision)| revision != u64::MAX) && flags & 1 == 0
            );
            value["text_source"] = serde_json::json!("since");
        }
        reply.json = value.to_string();
        Ok(success(reply.json))
    }
}
