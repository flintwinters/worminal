//! Application state and local transport shared by the Rust service and frontend.
pub mod browser_target;
pub mod handoff;
pub mod launch;
pub mod recovery;
pub mod rights;
pub mod socket;
pub mod ssh;
pub use recovery::format::{PROCESS_SHELL, PROCESS_STTY, PROCESS_TERM_NAME};
pub use recovery::format::{WORKSPACE_BROWSER_HOME, WORKSPACE_BROWSER_SCHEME};
