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
pub enum OpenMode {
    Detached,
    Subscribe,
    ReuseRestored,
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
