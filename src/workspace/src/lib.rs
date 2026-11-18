//! Application state and local transport shared by the Rust service and frontend.
pub mod broker;
pub mod browser_target;
pub mod cli_help;
pub mod cli_output;
pub mod cli_schema;
pub mod devices;
pub mod diagnostics;
pub mod gui_handoff;
pub mod handoff;
pub mod launch;
pub mod masters;
pub mod recovery;
pub mod rights;
pub mod socket;
pub mod spawn;
pub mod ssh;
mod statefile;
pub use browser_target::{WORKSPACE_BROWSER_HOME, WORKSPACE_BROWSER_SCHEME};
