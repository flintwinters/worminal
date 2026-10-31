//! Headless workspace application. OS ownership is isolated from terminal semantics.
mod cli;
mod commands;
mod frames;
mod handoff;
mod handoff_io;
pub mod process;
pub mod queue;
mod reactor;
mod service;
mod service_handoff;
mod service_io;
mod state;
pub use cli::run;
pub use service::serve;
