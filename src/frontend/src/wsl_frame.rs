//! Under WSLg, Weston's X11 window manager draws a white title bar and grey border inside the Windows RAIL
//! window; Windows itself draws no caption (so DWM caption colors have nothing to recolor). A GTK header bar
//! makes the frame client-side: Weston then leaves the window undecorated, and the bar is styled with the
//! terminal colors. Moving and resizing go through `_NET_WM_MOVERESIZE`, which Weston's XWM supports.
//! Anywhere else this does nothing.
use crate::theme;
use gtk::prelude::*;
use std::ffi::OsStr;

const CLASS: &str = "worminal-wsl-frame";

fn is_wsl(distro: Option<&OsStr>, osrelease: &str) -> bool {
    distro.is_some() || osrelease.to_ascii_lowercase().contains("microsoft")
}
fn detected() -> bool {
    let osrelease = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
    is_wsl(std::env::var_os("WSL_DISTRO_NAME").as_deref(), &osrelease)
}
/// Flat, compact chrome in the terminal colors; configuration colors are validated `#rrggbb`.
fn css(background: &str, foreground: &str) -> String {
    format!(
        "window.{CLASS} decoration {{ box-shadow: none; border-radius: 0; margin: 0; border: 1px solid {background}; }}
window.{CLASS} headerbar {{ background: {background}; color: {foreground}; border: none; border-radius: 0;
  box-shadow: none; min-height: 0; padding: 0 2px; }}
window.{CLASS} headerbar .title {{ color: {foreground}; font-weight: normal; }}
window.{CLASS} headerbar button {{ color: {foreground}; background: none; border: none; border-radius: 0;
  box-shadow: none; min-height: 0; min-width: 0; padding: 4px 10px; -gtk-icon-shadow: none; }}
window.{CLASS} headerbar button:hover {{ background: alpha({foreground}, 0.15); }}
window.{CLASS} headerbar button.close:hover {{ background: #c42b1c; color: #ffffff; }}"
    )
}

/// Returns the window so construction can end with the call.
pub(crate) fn style(window: gtk::Window) -> gtk::Window {
    if !worminal_config::get().wsl_window_frame || !detected() {
        return window;
    }
    let (background, foreground) = theme::colors();
    theme::install(CLASS, &css(&background, &foreground));
    window.style_context().add_class(CLASS);
    let bar = gtk::HeaderBar::new();
    bar.set_show_close_button(true);
    bar.set_title(window.title().as_deref());
    // A custom titlebar does not follow later window titles (tab and shell changes) by itself.
    window.connect_title_notify({
        let bar = bar.clone();
        move |window| bar.set_title(window.title().as_deref())
    });
    bar.show();
    window.set_titlebar(Some(&bar));
    window
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wsl_is_the_distro_variable_or_a_microsoft_kernel() {
        assert!(is_wsl(Some(OsStr::new("Ubuntu")), "6.6.0-generic"));
        assert!(is_wsl(None, "5.15.153.1-microsoft-standard-WSL2"));
        assert!(!is_wsl(None, "7.3.0-10-generic"));
    }
    #[test]
    fn the_frame_css_parses_with_terminal_colors() {
        crate::with_gtk(|| {
            gtk::CssProvider::new()
                .load_from_data(css("#181818", "#d8d8d8").as_bytes())
                .expect("frame CSS parses");
        });
    }
}
