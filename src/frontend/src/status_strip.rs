//! The message strip at the bottom of a window: a flat status rail in the terminal colors with a
//! hairline top rule and small monospaced text (UI.md), instead of GTK's stock label.
use crate::theme;
use gtk::prelude::*;

const CLASS: &str = "worminal-status";

fn css(background: &str, foreground: &str) -> String {
    format!(
        ".{CLASS} {{ background: {background}; color: {foreground}; border: none;
  border-top: 1px solid alpha({foreground}, 0.25); border-radius: 0; box-shadow: none;
  padding: 2px 8px; font-family: monospace; font-size: 9pt; }}"
    )
}

/// An ellipsized, initially hidden label styled as the status rail.
pub(crate) fn label() -> gtk::Label {
    let status = gtk::Label::new(None);
    status.set_ellipsize(gtk::pango::EllipsizeMode::End);
    status.set_xalign(0.0);
    status.set_no_show_all(true);
    let (background, foreground) = theme::colors();
    theme::install(CLASS, &css(&background, &foreground));
    status.style_context().add_class(CLASS);
    status
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_rail_css_parses_with_terminal_colors() {
        crate::with_gtk(|| {
            gtk::CssProvider::new()
                .load_from_data(css("#181818", "#d8d8d8").as_bytes())
                .expect("status CSS parses");
        });
    }
}
