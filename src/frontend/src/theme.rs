//! Screen-wide CSS for Worminal's own GTK widgets, written in the configured terminal colors.
//! GTK's stock theme draws nothing Worminal-like, so every widget that is not the terminal itself
//! takes a style class and a sheet installed here.
use gtk::prelude::*;
use std::{cell::RefCell, collections::HashSet};

const BACKGROUND: usize = 259;
const FOREGROUND: usize = 258;
/// Gruvbox Dark, the project palette, when the configuration leaves the terminal colors unset.
const DEFAULT_BACKGROUND: &str = "#282828";
const DEFAULT_FOREGROUND: &str = "#ebdbb2";

thread_local! { static INSTALLED: RefCell<HashSet<&'static str>> = RefCell::default(); }

/// The configured terminal background and foreground (validated `#rrggbb`).
pub(crate) fn colors() -> (String, String) {
    let settings = worminal_config::get();
    let color = |index: usize, default: &str| {
        settings.colors[index]
            .clone()
            .unwrap_or_else(|| default.to_string())
    };
    (
        color(BACKGROUND, DEFAULT_BACKGROUND),
        color(FOREGROUND, DEFAULT_FOREGROUND),
    )
}

/// Installs `css` for the whole screen once per `name`; returns whether it is in effect.
pub(crate) fn install(name: &'static str, css: &str) -> bool {
    if INSTALLED.with(|done| done.borrow().contains(name)) {
        return true;
    }
    let provider = gtk::CssProvider::new();
    let Some(screen) = gtk::gdk::Screen::default() else {
        return false;
    };
    if provider.load_from_data(css.as_bytes()).is_err() {
        return false;
    }
    gtk::StyleContext::add_provider_for_screen(
        &screen,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    INSTALLED.with(|done| done.borrow_mut().insert(name));
    true
}
