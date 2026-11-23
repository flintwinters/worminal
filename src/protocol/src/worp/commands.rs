//! Commands available to every Worp client. Selection, ownership and receipts are explicit policies.
use super::{launch::Launch, Request};
use std::io;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Acquire,
    Preserve,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlAction {
    TogglePrinter,
    PrintScreen,
    Break,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionOperation {
    Start,
    Next,
    Cancel,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseAction {
    Press,
    Release,
    Motion,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenMode {
    Detached,
    Subscribe,
    ReuseRestored,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MasterOperation {
    /// Connect (or retry now), letting SSH ask for a password.
    Attach,
    /// Connect without prompting: restored attachments and automatic retries.
    AttachQuietly,
    /// Stop showing the master; its tabs keep running there.
    Detach,
    /// Close every tab of the master.
    Close,
}

/// An SSH destination: never empty, never an option, at most 255 bytes.
pub fn valid_master(host: &[u8]) -> bool {
    !host.is_empty()
        && host.len() <= 255
        && host[0] != b'-'
        && host.iter().all(|byte| byte.is_ascii_graphic())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub tab: u32,
    pub kind: CommandKind,
}
impl Command {
    /// Validate before delivery so rejected commands cannot partially mutate a session.
    pub fn validate(&self) -> io::Result<()> {
        let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "Invalid Worp command");
        match &self.kind {
            CommandKind::Call(request) => request.validate()?,
            CommandKind::Master { host, .. } if !valid_master(host) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Invalid master: name one SSH destination without spaces, up to 255 bytes.",
                ))
            }
            CommandKind::Master {
                operation: MasterOperation::Detach | MasterOperation::Close,
                launch: Some(_),
                ..
            } => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Only a master attachment opens a terminal.",
                ))
            }
            CommandKind::Master {
                launch: Some(launch),
                ..
            } => {
                launch.encode()?;
            }
            CommandKind::Open { launch, .. } => {
                launch.encode()?;
            }
            CommandKind::Browser { url, .. }
            | CommandKind::Navigate(url)
            | CommandKind::ReplaceBrowser(url)
                if url.is_empty() || url.len() > super::MAX_INPUT_BYTES || url.contains(&0) =>
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Invalid browser URL: supply one nonempty URL up to 65536 bytes.",
                ))
            }
            CommandKind::Input { bytes, .. } if bytes.len() > super::MAX_INPUT_BYTES => {
                return Err(invalid())
            }
            CommandKind::Paste { bytes, .. } if bytes.len() > crate::input::MAX_PASTE_BYTES => {
                return Err(invalid())
            }
            // NUL encodes "no character" on the wire.
            CommandKind::Key {
                character: Some('\0'),
                ..
            } => return Err(invalid()),
            // The engine takes a signed row count; a wrapped amount would scroll a page the wrong way.
            CommandKind::Scroll { amount: lines, .. } | CommandKind::Wheel { lines, .. }
                if *lines > i32::MAX as u32 =>
            {
                return Err(invalid())
            }
            CommandKind::Wheel { notches, .. } if *notches > crate::input::MAX_WHEEL_NOTCHES => {
                return Err(invalid())
            }
            CommandKind::Focus { columns, rows }
            | CommandKind::Resize { columns, rows }
            | CommandKind::Pane { columns, rows }
                if *columns == 0
                    || *columns > crate::WORKSPACE_MAX_COLS
                    || *rows == 0
                    || *rows > crate::WORKSPACE_MAX_ROWS =>
            {
                return Err(invalid())
            }
            _ => {}
        }
        Ok(())
    }
    pub fn new(tab: u32, kind: CommandKind) -> Self {
        Self { tab, kind }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandKind {
    /// A correlated query/mutation with a JSON receipt; available on persistent sessions too.
    Call(Request),
    /// Attach, detach or close a master daemon by its SSH endpoint; see `WIRE_MASTER`. An attachment may
    /// carry a terminal to open there, which is created as `Open` with `OpenMode::Subscribe` would be.
    Master {
        operation: MasterOperation,
        host: Vec<u8>,
        launch: Option<Box<Launch>>,
    },
    Open {
        launch: Launch,
        mode: OpenMode,
    },
    Browser {
        url: Vec<u8>,
        subscribe: bool,
    },
    Navigate(Vec<u8>),
    ReplaceBrowser(Vec<u8>),
    Input {
        bytes: Vec<u8>,
        control: Control,
    },
    Focus {
        columns: u32,
        rows: u32,
    },
    Resize {
        columns: u32,
        rows: u32,
    },
    /// Focus that keeps the view's other shown tabs shown: a split pane. Needs `WIRE_FEATURE_PANES`.
    Pane {
        columns: u32,
        rows: u32,
    },
    FocusState(bool),
    Close {
        receipt: bool,
    },
    Scroll {
        up: bool,
        amount: u32,
    },
    /// Semantic input, encoded by the daemon against the tab's live modes; each acquires control
    /// as `Input` does. Needs `WIRE_FEATURE_INPUT`. Modifiers are `crate::input::MOD_*` masks.
    /// An X11 keysym with the character it produces (Ctrl and Alt combinations derive from it);
    /// views send only keys that encode under the modes they display, leaving the rest to their
    /// input method.
    Key {
        keysym: u32,
        modifiers: u32,
        character: Option<char>,
        numlock: bool,
    },
    /// A report for an application that owns the mouse; `button` is a `crate::input` button code.
    /// Dropped when the live modes request no such report.
    Mouse {
        button: u32,
        column: u32,
        row: u32,
        modifiers: u32,
        action: MouseAction,
    },
    /// One chunk of at most `MAX_PASTE_BYTES` that never separates CR from a following LF:
    /// filtered, and bracketed at `begin`/`end` when the application asks for bracketed paste.
    Paste {
        bytes: Vec<u8>,
        begin: bool,
        end: bool,
    },
    /// Wheel notches: mouse reports for an application owning the mouse (unless Shift is held),
    /// arrow keys on an alternate screen with alternate scroll, otherwise `lines` of history.
    Wheel {
        up: bool,
        notches: u32,
        lines: u32,
        column: u32,
        row: u32,
        modifiers: u32,
    },
    Control(ControlAction),
    Selection {
        generation: u32,
        operation: SelectionOperation,
    },
    /// Reorder: the tab takes this zero-based position among the daemon's tabs.
    Move {
        position: u32,
    },
    Release,
    /// The sending view alerted this tab at this severity; the daemon relays it to the other views.
    Attention(u32),
    Subscribe,
    Capabilities(u32),
    ServiceInfo,
    Stop,
    Transfer {
        outbound: bool,
        token: Vec<u8>,
    },
}
