//! Application state and local transport shared by the Rust service and frontend.
pub mod launch;
pub mod recovery;
pub mod rights;
pub mod socket;
pub use recovery::format::{PROCESS_SHELL, PROCESS_STTY, PROCESS_TERM_NAME};
