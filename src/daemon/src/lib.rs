//! Headless workspace application. OS ownership is isolated from terminal semantics.
mod api;
mod api_cli;
mod cli;
mod commands;
mod config_cli;
mod config_sync;
mod device_probes;
mod frames;
mod handoff;
mod handoff_io;
pub mod process;
pub mod queue;
mod reactor;
mod remote_install;
mod service;
mod service_handoff;
mod service_io;
mod service_upgrade;
mod state;
mod upgrade_io;
pub use cli::run;
pub use service::serve;

#[cfg(test)]
mod perf_tests;
