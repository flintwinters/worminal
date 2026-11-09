//! Worp application requests are independent of their communication medium.
//!
//! A transport delivers one request and its reply. A handler owns application state;
//! neither knows about socket addresses, SSH processes, or packet framing. Adapters
//! own delivery, authentication and deadlines. They must not silently retry input.
use std::io;

pub mod devices;
pub mod stream;
pub mod v2;

pub const MAX_INPUT_BYTES: usize = 65536;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    List,
    Devices,
    Enroll { device: devices::Device },
    Forget { name: String },
    Read { tab: u32 },
    Input { tab: u32, bytes: Vec<u8> },
}

impl Request {
    pub fn tab(&self) -> u32 {
        match self {
            Self::List | Self::Devices | Self::Enroll { .. } | Self::Forget { .. } => 0,
            Self::Read { tab } | Self::Input { tab, .. } => *tab,
        }
    }

    /// Validate at both delivery boundaries, before a handler can mutate state.
    pub fn validate(&self) -> io::Result<()> {
        match self {
            Self::Enroll { device } => device.validate()?,
            Self::Forget { name } => devices::validate_name(name)?,
            _ => {}
        }
        if matches!(self, Self::Read { .. } | Self::Input { .. }) && self.tab() == 0 {
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
