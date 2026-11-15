//! Worp commands and events are independent of their communication medium and client interface.
//! A persistent session carries subscriptions, viewport deltas, input and lifecycle operations.
//! Request/Reply is the unary convenience form of the same operations. Delivery adapters own
//! authentication and deadlines; they never silently retry mutations.
use std::io;

pub mod commands;
pub mod devices;
pub mod events;
pub mod launch;
pub mod session;
pub mod stream;
pub mod tabs;
pub mod v2;
mod v2_agent;
pub mod v2_commands;
pub mod v2_events;

pub const MAX_INPUT_BYTES: usize = 65536;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    List,
    ReadMany {
        tabs: Vec<u32>,
        tail: u32,
    },
    Keys {
        tab: u32,
        names: Vec<String>,
        guard: String,
    },
    Paste {
        tab: u32,
        text: String,
        guard: String,
    },
    GuardedInput {
        tab: u32,
        bytes: Vec<u8>,
        guard: String,
    },
    Wait {
        tab: u32,
        after: String,
        timeout_ms: u32,
    },
    OpenTerminal {
        options: tabs::TerminalOptions,
    },
    OpenBrowser {
        url: String,
    },
    Close {
        tab: u32,
    },
    Devices,
    Enroll {
        device: devices::Device,
    },
    Forget {
        name: String,
    },
    Read {
        tab: u32,
    },
    Input {
        tab: u32,
        bytes: Vec<u8>,
    },
}

impl Request {
    pub fn tab(&self) -> u32 {
        match self {
            Self::List
            | Self::ReadMany { .. }
            | Self::OpenTerminal { .. }
            | Self::OpenBrowser { .. }
            | Self::Devices
            | Self::Enroll { .. }
            | Self::Forget { .. } => 0,
            Self::Keys { tab, .. }
            | Self::Paste { tab, .. }
            | Self::GuardedInput { tab, .. }
            | Self::Wait { tab, .. }
            | Self::Read { tab }
            | Self::Input { tab, .. }
            | Self::Close { tab } => *tab,
        }
    }

    /// Validate at both delivery boundaries, before a handler can mutate state.
    pub fn validate(&self) -> io::Result<()> {
        if let Self::Paste { text, .. } = self {
            if text
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Paste permits only text, tab and line endings.",
                ));
            }
        }
        match self {
            Self::OpenTerminal { options } => options.validate()?,
            Self::OpenBrowser { url } => tabs::validate_url(url)?,
            Self::Enroll { device } => device.validate()?,
            Self::Forget { name } => devices::validate_name(name)?,
            Self::ReadMany { tabs, tail }
                if tabs.is_empty() || tabs.len() > 16 || tabs.contains(&0) || *tail > 1000 =>
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Read requires 1..16 positive tab IDs; --tail is 1..1000.",
                ))
            }
            Self::Wait {
                after, timeout_ms, ..
            } if after.len() > 256 || *timeout_ms > 60000 => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Wait timeout is 0..60 seconds; revision at most 256 bytes.",
                ))
            }
            Self::Keys { names, guard, .. }
                if names.is_empty()
                    || names.len() > 256
                    || names.iter().any(|n| n.len() > 64)
                    || guard.len() > 1024 =>
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Keys require 1..256 names of at most 64 bytes; guard at most 1024 bytes.",
                ))
            }
            Self::Paste { text, guard, .. }
                if text.len() > MAX_INPUT_BYTES || guard.len() > 1024 =>
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Paste exceeds 65536 bytes or guard exceeds 1024 bytes.",
                ))
            }
            Self::GuardedInput { bytes, guard, .. }
                if bytes.len() > MAX_INPUT_BYTES || guard.len() > 1024 =>
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Input exceeds 65536 bytes or guard exceeds 1024 bytes.",
                ))
            }
            _ => {}
        }
        if matches!(
            self,
            Self::Read { .. }
                | Self::Input { .. }
                | Self::Close { .. }
                | Self::Keys { .. }
                | Self::Paste { .. }
                | Self::GuardedInput { .. }
                | Self::Wait { .. }
        ) && self.tab() == 0
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Tab ID must be positive.",
            ));
        }
        if matches!(self, Self::Input { bytes, .. } if bytes.len() > MAX_INPUT_BYTES) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Input exceeds 65536 bytes.",
            ));
        }
        Ok(())
    }
}

/// The JSON application document remains identical across transports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub success: bool,
    pub json: String,
}

pub trait Handler {
    fn handle(&mut self, request: Request) -> io::Result<Reply>;
}

pub trait Transport {
    /// On uncertain delivery return an error; never automatically repeat an action.
    fn exchange(&mut self, request: Request) -> io::Result<Reply>;
}

pub struct Client<T> {
    transport: T,
}

impl<T: Transport> Client<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    pub fn request(&mut self, request: Request) -> io::Result<Reply> {
        request.validate()?;
        self.transport.exchange(request)
    }
}

/// Direct delivery is useful to embedders and proves that Worp needs no byte stream.
pub struct Direct<T>(pub T);

impl<T: Handler> Transport for Direct<T> {
    fn exchange(&mut self, request: Request) -> io::Result<Reply> {
        request.validate()?;
        self.0.handle(request)
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod command_tests;
