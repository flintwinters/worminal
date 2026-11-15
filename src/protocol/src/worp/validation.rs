//! Shared validation for unary observation and input extensions.
use super::{Request, MAX_INPUT_BYTES};
use std::io;
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
fn keys(names: &[String], guard: &str) -> io::Result<()> {
    if names.is_empty()
        || names.len() > 256
        || names.iter().any(|n| n.len() > 64)
        || guard.len() > 1024
    {
        return Err(invalid(
            "Keys require 1..256 names of at most 64 bytes; guard at most 1024 bytes.",
        ));
    }
    Ok(())
}
pub(super) fn agent(request: &Request) -> io::Result<()> {
    match request {
        Request::Paste { text, guard, .. } | Request::PasteKeys { text, guard, .. } => {
            if text.len() > MAX_INPUT_BYTES || guard.len() > 1024 {
                return Err(invalid(
                    "Paste exceeds 65536 bytes or guard exceeds 1024 bytes.",
                ));
            }
            if text
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
            {
                return Err(invalid("Paste permits only text, tab and line endings."));
            }
            if let Request::PasteKeys { names, .. } = request {
                keys(names, guard)?;
            }
        }
        Request::Keys { names, guard, .. } => keys(names, guard)?,
        Request::GuardedInput { bytes, guard, .. }
            if bytes.len() > MAX_INPUT_BYTES || guard.len() > 1024 =>
        {
            return Err(invalid(
                "Input exceeds 65536 bytes or guard exceeds 1024 bytes.",
            ))
        }
        Request::Wait {
            after, timeout_ms, ..
        } if after.len() > 256 || *timeout_ms > 60000 => {
            return Err(invalid(
                "Wait timeout is 0..60 seconds; revision at most 256 bytes.",
            ))
        }
        Request::WaitCommand {
            workspace,
            command_id,
            timeout_ms,
            ..
        } if workspace.is_empty()
            || workspace.len() > 256
            || *command_id == 0
            || *timeout_ms > 60000 =>
        {
            return Err(invalid(
                "Command wait needs workspace, positive command ID and timeout 0..60 seconds.",
            ))
        }
        Request::ReadOutput {
            since, command_id, ..
        } if since.len() > 256 || (since.is_empty() == (*command_id == 0)) => {
            return Err(invalid(
                "Output read needs either a cursor or positive command ID.",
            ))
        }
        _ => {}
    }
    Ok(())
}
