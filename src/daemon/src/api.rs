//! Unary convenience commands over the shared, same-user Worp session.
use crate::{
    service::Service,
    state::{core, Tab},
};
use std::io;
use worminal_protocol::worp::{Handler, Reply, Request};

#[path = "api_tabs.rs"]
mod tabs;

pub fn help() -> String {
    use worminal_workspace::cli_schema::{Api, ApiOption};
    format!("Worp: Worminal command and event API (version 1)\n\
Usage: worminald api [--master user@host] COMMAND\n{}\nOptions (where supported by the command):\n{}\n{}",
        Api::GROUP.reference(), ApiOption::GROUP.reference(), HELP_DETAILS)
}

const HELP_DETAILS: &str = r#"Examples:
  worminald api list
  worminald api open --cwd /path/to/project -- /bin/sh
  worminald api open --browser https://example.org/
  worminald api close 12
  worminald api read 12
  worminald api type 12 'What are you working on?'
  worminald api key 12 Enter
Registry authority: worminald api --master user@host devices
Remote: worminald api --master user@host list
Bootstrap a new node: worminald install-on-remote --help
Worp transports: authenticated local socket or SSH; identical semantic commands and events.
Persistent session (binary): worminald api --session, or ssh user@host worminald api --session.
Reply JSON keys, limits and exit codes: worminald api help schema. Streaming wire format: worminald api help session.
Unary commands below are a JSON request/reply convenience; session clients may use them too.
All operations connect to an existing daemon; none starts or restores a workspace.
Open does not open a GUI window or take control. Paths, argv and environment belong to the target daemon.
Command arguments are passed literally; no implicit shell interpretation or caller environment forwarding.
Browser records load only when displayed. Close removes the tab and initiates terminal hangup, without confirmation.
Open/close are mutations; do not automatically retry uncertain replies. List again to inspect the outcome.
Input targets a tab, preserves its view/controller and size, and may execute commands.
Input success means queued for the PTY, not that the application processed it.
TAB accepts self for local caller ancestry (including batch reads); --master refuses self.
Tab IDs belong to the current daemon workspace; list again after a restart.
WORMINAL_SHARED_SOCKET_SCOPE selects the same workspace as graphical clients.
Enrollment is per authority user across scopes; no heartbeats or automatic enrollment.
"#;

pub fn quote(bytes: &[u8]) -> String {
    serde_json::to_string(&String::from_utf8_lossy(bytes)).expect("UTF-8 strings serialize to JSON")
}

fn record(tab: &mut Tab, now: i64, workspace: &str) -> String {
    tab.refresh_directory(now);
    let pid = tab
        .terminal
        .as_mut()
        .map_or(0, |terminal| terminal.pty.live_pid());
    format!(
        "{{\"id\":{},\"kind\":{},\"title\":{},\"directory\":{},\"shell_pid\":{},\"url\":{},\"host\":{},\"state\":{}}}",
        tab.id,
        quote(if tab.terminal.is_some() {
            b"terminal"
        } else if tab.remote.is_some() {
            b"master"
        } else {
            b"browser"
        }),
        quote(&tab.title),
        quote(&tab.directory),
        pid,
        quote(tab.url.as_deref().unwrap_or_default()),
        quote(tab.remote.as_ref().map_or(&[][..], |remote| &remote.host)),
        crate::api_observe::state(tab, workspace)
    )
}

pub(crate) fn read(tab: &mut Tab, now: i64) -> io::Result<String> {
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
    failure_code("operation_failed", message, None)
}

pub(crate) use crate::api_cli::diagnostics::failure as failure_code;

pub(crate) fn success(json: String) -> Reply {
    Reply {
        success: true,
        json,
    }
}

impl Handler for Service {
    fn handle(&mut self, request: Request) -> io::Result<Reply> {
        if let Err(error) = request.validate() {
            return Ok(failure_code(
                "invalid_request",
                &error.to_string(),
                Some("Run api help."),
            ));
        }
        if let Some(reply) = self.tab_operation(&request, self.now())? {
            return Ok(reply);
        }
        if let Some(reply) = self.extended(&request, self.now())? {
            return Ok(reply);
        }
        match &request {
            Request::Devices => {
                let devices = match self.devices.list() {
                    Ok(devices) => devices,
                    Err(error) => return Ok(failure(&error.to_string())),
                };
                let devices: Vec<_> = devices.iter().map(|device| serde_json::json!({
                    "name": device.name, "endpoint": device.endpoint, "availability": "unchecked"
                })).collect();
                return Ok(success(
                    serde_json::json!({"ok": true, "devices": devices}).to_string(),
                ));
            }
            Request::Enroll { device } => {
                return Ok(match self.devices.enroll(device.clone()) {
                    Ok(changed) => success(
                        serde_json::json!({"ok":true, "enrolled": true,
                        "name":device.name, "endpoint":device.endpoint, "changed":changed})
                        .to_string(),
                    ),
                    Err(error) => failure(&error.to_string()),
                });
            }
            Request::Forget { name } => {
                return Ok(match self.devices.forget(name) {
                    Ok(removed) => success(
                        serde_json::json!({"ok":true, "name":name, "removed":removed}).to_string(),
                    ),
                    Err(error) => failure(&error.to_string()),
                });
            }
            _ => {}
        }
        let now = self.now();
        if request == Request::List {
            let tabs: Vec<_> = self
                .tabs
                .iter_mut()
                .map(|tab| record(tab, now, &self.workspace_id))
                .collect();
            let json = format!(
                "{{\"ok\":true,\"api_version\":1,\"daemon_version\":{},\"workspace\":{},\"tabs\":[{}]}}",
                quote(env!("CARGO_PKG_VERSION").as_bytes()),
                quote(self.workspace_id.as_bytes()),
                tabs.join(",")
            );
            return Ok(success(json));
        }
        let tab = request.tab();
        let Some(index) = self.index(tab) else {
            return Ok(crate::api::failure_code(
                "tab_not_found",
                "Tab not found; run worminald api list.",
                Some("Run worminald api list."),
            ));
        };
        let Some(_) = self.tabs[index].terminal.as_ref() else {
            return Ok(crate::api::failure_code(
                "terminal_required",
                "This operation requires a terminal tab.",
                Some("Choose a terminal from api list."),
            ));
        };
        let Request::Input { bytes, .. } = request else {
            return self.observe(tab, 0, now);
        };
        self.input_operation(tab, &bytes, None, now)
    }
}

impl Service {
    pub(crate) fn input_operation(
        &mut self,
        tab: u32,
        bytes: &[u8],
        owner: Option<crate::state::ClientId>,
        now: i64,
    ) -> io::Result<Reply> {
        let Some(index) = self.index(tab) else {
            return Ok(crate::api::failure_code(
                "tab_not_found",
                "Tab not found; run worminald api list.",
                Some("Run worminald api list."),
            ));
        };
        let Some(terminal) = self.tabs[index].terminal.as_ref() else {
            return Ok(crate::api::failure_code(
                "terminal_required",
                "This operation requires a terminal tab.",
                Some("Choose a terminal from api list."),
            ));
        };
        // Engine LNM can double CR bytes. Detached writers reject overflow before mutation;
        // a controlling stream retains the established overload error/affected-tab stop policy.
        if bytes.len() > worminal_protocol::worp::MAX_INPUT_BYTES
            || (owner.is_none() && bytes.len() * 2 > terminal.input.remaining())
        {
            return Ok(failure(
                "Input exceeds 65536 bytes or the PTY queue is full.",
            ));
        }
        if owner.is_none() && terminal.pty.fd().is_none() {
            return Ok(crate::api::failure_code(
                "process_exited",
                "Terminal process has exited.",
                Some("Open a new terminal."),
            ));
        }
        // Acquiring control is explicit; detached input preserves the current viewport.
        if let Some(owner) = owner {
            self.take_control(index, owner);
        }
        // Input answers an alert in every view, however it arrived.
        self.set_attention(index, 0, None);
        let notices = self.tabs[index].input(bytes, true, now)?;
        self.notices(tab, notices);
        self.send_frame(index, false, now)?;
        Ok(success(format!(
            "{{\"ok\":true,\"id\":{},\"queued_bytes\":{}}}",
            tab,
            bytes.len()
        )))
    }
}
