//! Headless, same-user automation over the existing workspace connection.
use crate::{
    service::Service,
    state::{core, Tab},
};
use std::io;
use worminal_protocol::worp::{Handler, Reply, Request};

pub const HELP: &str = r#"Worp: Worminal automation API (version 1)
Usage: worminald api [--master user@host] COMMAND
  help                 Show this command reference; needs no running daemon.
  list                 List tab IDs, titles, directories and shell PIDs as JSON.
  read TAB             Read the current viewport as JSON, without taking control.
  type TAB TEXT        Send literal UTF-8 text; does not append Enter.
  key TAB KEY...       Send Enter, Tab, Escape, Backspace, or Ctrl-A through Ctrl-Z.
Examples:
  worminald api list
  worminald api read 12
  worminald api type 12 'What are you working on?'
  worminald api key 12 Enter
Remote: worminald api --master user@host list
Worp transports: authenticated local socket or SSH; same operations and JSON replies.
All operations connect to an existing daemon; none starts or restores a workspace.
Input targets a tab, preserves its view/controller and size, and may execute commands.
Input success means queued for the PTY, not that the application processed it.
Tab IDs belong to the current daemon workspace; list again after a restart.
WORMINAL_SHARED_SOCKET_SCOPE selects the same workspace as graphical clients.
"#;

pub fn quote(bytes: &[u8]) -> String {
    let mut result = String::from("\"");
    for ch in String::from_utf8_lossy(bytes).chars() {
        match ch {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            ch if ch < ' ' => result.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => result.push(ch),
        }
    }
    result.push('"');
    result
}

fn record(tab: &mut Tab, now: i64) -> String {
    tab.refresh_directory(now);
    let pid = tab
        .terminal
        .as_mut()
        .map_or(0, |terminal| terminal.pty.live_pid());
    format!(
        "{{\"id\":{},\"kind\":{},\"title\":{},\"directory\":{},\"shell_pid\":{},\"url\":{}}}",
        tab.id,
        quote(if tab.terminal.is_some() {
            b"terminal"
        } else {
            b"browser"
        }),
        quote(&tab.title),
        quote(&tab.directory),
        pid,
        quote(tab.url.as_deref().unwrap_or_default())
    )
}

fn read(tab: &mut Tab, now: i64) -> io::Result<String> {
    let terminal = tab.terminal.as_mut().unwrap();
    let frame = core(terminal.engine.frame(now))?;
    let mut cells = Vec::new();
    let mut lines = Vec::new();
    for row in 0..frame.rows {
        core(terminal.engine.row(row, &mut cells))?;
        let line: String = cells
            .iter()
            .filter(|cell| cell.attributes & worminal_engine::ATTR_WDUMMY == 0)
            .map(|cell| {
                char::from_u32(cell.rune)
                    .filter(|ch| *ch != '\0')
                    .unwrap_or(' ')
            })
            .collect();
        lines.push(quote(line.trim_end_matches(' ').as_bytes()));
    }
    Ok(format!(
        "{{\"ok\":true,\"id\":{},\"columns\":{},\"rows\":{},\"cursor\":{{\"x\":{},\"y\":{}}},\
        \"scroll\":{},\"alternate\":{},\"lines\":[{}]}}",
        tab.id,
        frame.columns,
        frame.rows,
        frame.cursor_x,
        frame.cursor_y,
        frame.scroll,
        frame.alternate != 0,
        lines.join(",")
    ))
}

pub fn failure(message: &str) -> Reply {
    Reply {
        success: false,
        json: format!("{{\"ok\":false,\"error\":{}}}", quote(message.as_bytes())),
    }
}

fn success(json: String) -> Reply {
    Reply {
        success: true,
        json,
    }
}

impl Handler for Service {
    fn handle(&mut self, request: Request) -> io::Result<Reply> {
        if let Err(error) = request.validate() {
            return Ok(failure(&error.to_string()));
        }
        let now = self.now();
        if request == Request::List {
            let tabs: Vec<_> = self.tabs.iter_mut().map(|tab| record(tab, now)).collect();
            let json = format!(
                "{{\"ok\":true,\"api_version\":1,\"tabs\":[{}]}}",
                tabs.join(",")
            );
            return Ok(success(json));
        }
        let tab = request.tab();
        let Some(index) = self.index(tab) else {
            return Ok(failure("Tab not found; run worminald api list."));
        };
        let Some(terminal) = self.tabs[index].terminal.as_ref() else {
            return Ok(failure("This operation requires a terminal tab."));
        };
        let Request::Input { bytes, .. } = request else {
            return Ok(success(read(&mut self.tabs[index], now)?));
        };
        // Engine LNM can expand each CR into CRLF. Reject before engine input owns any
        // mutation: overflowing Tab::effects otherwise stops the shell to bound memory.
        if bytes.len() * 2 > terminal.input.remaining() {
            return Ok(failure(
                "Input exceeds 65536 bytes or the PTY queue is full.",
            ));
        }
        if terminal.pty.fd().is_none() {
            return Ok(failure("Terminal process has exited."));
        }
        // Unlike a graphical input source, one-shot automation must not steal the live
        // controller: disconnecting would otherwise leave its painting view detached.
        let notices = self.tabs[index].input(&bytes, true, now)?;
        self.notices(tab, notices);
        self.send_frame(index, false, now)?;
        Ok(success(format!(
            "{{\"ok\":true,\"id\":{},\"queued_bytes\":{}}}",
            tab,
            bytes.len()
        )))
    }
}
