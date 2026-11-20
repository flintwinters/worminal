//! Rust-owned frontend state. The release module remains C until its replacement is complete.
pub mod app;
mod askpass;
mod boundary;
mod browser;
pub mod fonts;
mod graphics_ffi;
pub mod input;
pub mod layout;
mod link;
mod link_action;
pub mod paint;
pub mod panel;
pub mod panel_tabs;
mod startup;
pub mod tabs;
pub mod terminal;
pub mod viewport;
mod wsl_frame;

pub(crate) fn timing(event: &str) {
    if std::env::var_os("WORMINAL_TRACE_TIMING").is_some() {
        eprintln!("worminal-timing {event} {}", glib::monotonic_time() * 1000);
    }
}
