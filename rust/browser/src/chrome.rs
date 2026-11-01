//! Native location/find/error chrome. Search presentation reuses the established page script.
use crate::Browser;
use gtk::{
    gdk::{keys::constants as key, ModifierType as Mod},
    gio,
    prelude::*,
};
use std::rc::Rc;
use webkit2gtk::*;

fn present(browser: &Browser, action: &str) {
    let script = format!(
        "{}\nWorminalFind.update('{action}');",
        include_str!("../../../src/gtk/browser-find.js")
    );
    browser.view.evaluate_javascript(
        &script,
        Some("worminal-find"),
        None,
        gio::Cancellable::NONE,
        |_| {},
    );
}
fn completed(browser: &Browser) {
    if browser.select_find.replace(false) && browser.find.has_focus() {
        browser.find.select_region(0, -1);
    }
}
fn close(browser: &Browser) {
    browser.select_find.set(false);
    browser.find.hide();
    present(browser, "clear");
    browser
        .view
        .find_controller()
        .expect("find controller")
        .search_finish();
}
fn search(browser: &Browser) {
    browser.select_find.set(false);
    present(browser, "clear");
    browser.find.style_context().remove_class("error");
    browser.find.set_tooltip_text(Some(
        "Enter: next match; Shift+Enter: previous; Escape: select result",
    ));
    let controller = browser.view.find_controller().expect("find controller");
    let text = browser.find.text();
    if !gtk::prelude::WidgetExt::is_visible(&browser.find) || text.is_empty() {
        controller.search_finish();
    } else {
        controller.search(
            &text,
            (FindOptions::CASE_INSENSITIVE | FindOptions::WRAP_AROUND).bits(),
            u32::MAX,
        );
    }
}
pub(crate) fn connect(browser: &Rc<Browser>) {
    browser.location.connect_activate(glib::clone!(@weak browser => move |entry| {
        let text = entry.text();
        if text.is_empty() { return; }
        let url = if glib::Uri::parse_scheme(&text).is_some() { text.to_string() } else { format!("https://{text}") };
        browser.view.load_uri(&url); entry.hide(); browser.view.grab_focus();
    }));
    browser
        .find
        .connect_changed(glib::clone!(@weak browser => move |_| search(&browser)));
    let controller = browser.view.find_controller().expect("find controller");
    controller.connect_found_text(glib::clone!(@weak browser => move |_, _| {
        if gtk::prelude::WidgetExt::is_visible(&browser.find) { present(&browser, "show"); } completed(&browser);
    }));
    controller.connect_failed_to_find_text(glib::clone!(@weak browser => move |_| {
        completed(&browser);
        if gtk::prelude::WidgetExt::is_visible(&browser.find) && !browser.find.text().is_empty() {
            present(&browser, "clear"); browser.find.style_context().add_class("error");
            browser.find.set_tooltip_text(Some("No matches"));
        }
    }));
    browser
        .view
        .connect_title_notify(glib::clone!(@weak browser => move |_| browser.changed(false)));
    browser
        .view
        .connect_uri_notify(glib::clone!(@weak browser => move |_| browser.changed(true)));
    browser
        .view
        .connect_load_changed(glib::clone!(@weak browser => move |_, event| {
            if event == LoadEvent::Started { browser.notice(None); close(&browser); }
            browser.changed(false);
        }));
    browser.view.connect_load_failed(glib::clone!(@weak browser => @default-return true, move |view, _, url, error| {
        // Downloads/media hand off provisionally; alternate HTML would abort their loader.
        if !error.matches(NetworkError::Cancelled)
            && !error.matches(PolicyError::FrameLoadInterruptedByPolicyChange)
            && !error.matches(PluginError::WillHandleLoad) { error_page(view, url, error.message()); }
        true
    }));
    browser
        .view
        .connect_load_failed_with_tls_errors(|view, url, _, errors| {
            error_page(
                view,
                url,
                &format!(
                    "Certificate validation failed ({errors:?}) for {url}. Connection blocked."
                ),
            );
            true
        });
    browser
        .view
        .connect_web_process_terminated(glib::clone!(@weak browser => move |_, _| {
            browser.notice(Some("The page renderer stopped. Press Ctrl+R to reopen this page."));
        }));
}
fn error_page(view: &WebView, url: &str, message: &str) {
    let html = include_str!("../../../src/gtk/browser-error.html")
        .replace("{{message}}", &glib::markup_escape_text(message));
    view.load_alternate_html(&html, url, None);
}
pub(crate) fn shortcut(browser: &Browser, value: u32, modifiers: u32) -> bool {
    let value = gtk::gdk::keys::Key::from(value).to_lower();
    let mods =
        Mod::from_bits_truncate(modifiers) & (Mod::CONTROL_MASK | Mod::SHIFT_MASK | Mod::MOD1_MASK);
    if mods == Mod::CONTROL_MASK && value == key::f {
        browser.location.hide();
        browser.find.show();
        search(browser);
        browser.select_find.set(!browser.find.text().is_empty());
        browser.find.grab_focus();
        browser.find.select_region(0, -1);
    } else if gtk::prelude::WidgetExt::is_visible(&browser.find)
        && browser.find.has_focus()
        && [key::Return, key::KP_Enter].contains(&value)
        && (mods.is_empty() || mods == Mod::SHIFT_MASK)
    {
        if !browser.find.text().is_empty() {
            let controller = browser.view.find_controller().expect("find controller");
            if mods.is_empty() {
                controller.search_next();
            } else {
                controller.search_previous();
            }
        }
    } else if mods == Mod::CONTROL_MASK && value == key::k {
        close(browser);
        let uri = browser.view.uri().unwrap_or_default();
        browser
            .location
            .set_text(if uri == "about:blank" { "" } else { &uri });
        browser.location.show();
        browser.location.grab_focus();
        browser.location.select_region(0, -1);
    } else if mods == Mod::MOD1_MASK && [key::Left, key::Right].contains(&value) {
        if value == key::Left {
            browser.view.go_back();
        } else {
            browser.view.go_forward();
        }
    } else if (mods == Mod::CONTROL_MASK && value == key::r)
        || (mods.is_empty() && value == key::F5)
    {
        browser.notice(None);
        browser.view.reload();
    } else if mods.is_empty()
        && value == key::Escape
        && (gtk::prelude::WidgetExt::is_visible(&browser.find)
            || gtk::prelude::WidgetExt::is_visible(&browser.location)
            || browser.view.is_loading())
    {
        if gtk::prelude::WidgetExt::is_visible(&browser.find) {
            close(browser);
            present(browser, "accept");
        } else if gtk::prelude::WidgetExt::is_visible(&browser.location) {
            browser.location.hide();
        } else {
            browser.view.stop_loading();
        }
        browser.view.grab_focus();
    } else {
        return false;
    }
    true
}
