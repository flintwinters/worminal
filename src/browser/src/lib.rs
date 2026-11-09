//! WebKit objects and browser policy stay inside this lazy module.
mod boundary;
mod chrome;
mod local;
mod policy;
mod profile_transfer;

use gtk::{gio, prelude::*};
use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
};
use webkit2gtk::*;

thread_local! {
    static CONTEXT: RefCell<Option<WebContext>> = const { RefCell::new(None) };
    static VIEWS: RefCell<Vec<Weak<Browser>>> = const { RefCell::new(Vec::new()) };
    static RETIRED: Cell<bool> = const { Cell::new(false) };
}

pub(crate) struct Browser {
    container: gtk::Box,
    view: WebView,
    location: gtk::Entry,
    find: gtk::SearchEntry,
    notice: gtk::Label,
    select_find: Cell<bool>,
    url: RefCell<String>,
    callback: boundary::Callback,
}
impl Browser {
    fn new(profile: &WebsiteDataManager, url: &str, callback: boundary::Callback) -> Rc<Self> {
        let context = CONTEXT.with(|slot| {
            slot.borrow_mut()
                .get_or_insert_with(|| {
                    let context = WebContext::with_website_data_manager(profile);
                    local::register(&context);
                    context.set_spell_checking_enabled(false);
                    policy::downloads(&context);
                    context
                })
                .clone()
        });
        let content = UserContentManager::new();
        let mut builder = WebView::builder().user_content_manager(&content);
        if let Some(related) = related(url) {
            builder = builder.related_view(&related);
        } else {
            builder = builder.web_context(&context);
        }
        let view = builder.build();
        local::authorize(&view, &content, url);
        view.set_background_color(&gtk::gdk::RGBA::new(0.094, 0.094, 0.094, 1.0));
        view.set_settings(
            &Settings::builder()
                .enable_javascript(true)
                .auto_load_images(true)
                .default_charset("UTF-8")
                .enable_developer_extras(false)
                .enable_dns_prefetching(false)
                .media_playback_requires_user_gesture(true)
                .enable_smooth_scrolling(false)
                .enable_webgl(false)
                .javascript_can_open_windows_automatically(false)
                .build(),
        );
        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let location = gtk::Entry::new();
        location.set_placeholder_text(Some("URL"));
        let find = gtk::SearchEntry::new();
        find.set_placeholder_text(Some("Find in page"));
        let notice = gtk::Label::new(None);
        notice.set_line_wrap(true);
        for widget in [
            location.clone().upcast::<gtk::Widget>(),
            find.clone().upcast(),
            notice.clone().upcast(),
        ] {
            widget.set_no_show_all(true);
            container.pack_start(&widget, false, false, 0);
        }
        container.pack_start(&view, true, true, 0);
        let browser = Rc::new(Self {
            container,
            view,
            location,
            find,
            notice,
            select_find: Cell::new(false),
            url: RefCell::new(url.to_string()),
            callback,
        });
        chrome::connect(&browser);
        policy::connect(&browser);
        VIEWS.with(|views| views.borrow_mut().push(Rc::downgrade(&browser)));
        browser
    }
    fn emit(&self, event: u32, text: &str) {
        self.callback.emit(event, text);
    }
    fn notice(&self, text: Option<&str>) {
        self.notice.set_text(text.unwrap_or(""));
        self.notice.set_visible(text.is_some());
    }
    fn changed(&self, navigate: bool) {
        // WebKit clears its URI when a renderer crashes. Preserve the last real navigation
        // here so location chrome and explicit recovery never replace it with an empty target.
        if let Some(uri) = self.view.uri().filter(|uri| !uri.is_empty()) {
            *self.url.borrow_mut() = uri.to_string();
        }
        let uri = self.url.borrow().clone();
        if !self.location.has_focus() {
            self.location.set_text(&uri);
        }
        let title = self
            .view
            .title()
            .filter(|title| !title.is_empty())
            .map(|title| title.to_string())
            .unwrap_or_else(|| uri.clone());
        self.emit(worminal_browser_api::WORMINAL_BROWSER_TITLE, &title);
        if navigate {
            self.emit(worminal_browser_api::WORMINAL_BROWSER_NAVIGATE, &uri);
        }
    }
}
fn related(url: &str) -> Option<WebView> {
    let origin = SecurityOrigin::for_uri(url);
    let document = local::document(url);
    if document.is_none() && !matches!(origin.protocol().as_deref(), Some("http" | "https")) {
        return None;
    }
    VIEWS.with(|views| {
        views
            .borrow()
            .iter()
            .filter_map(Weak::upgrade)
            .find_map(|browser| {
                let uri = browser.view.uri()?;
                let same = if let Some(document) = &document {
                    local::document(&uri).is_some_and(|other| {
                        gio::File::for_uri(document) == gio::File::for_uri(&other)
                    })
                } else {
                    let other = SecurityOrigin::for_uri(&uri);
                    origin.protocol() == other.protocol()
                        && origin.host() == other.host()
                        && origin.port() == other.port()
                };
                same.then(|| browser.view.clone())
            })
    })
}
fn retire(value: u32) {
    RETIRED.with(|retired| retired.set(value != 0));
    release_context();
}
fn release_context() {
    let empty = VIEWS.with(|views| {
        let mut views = views.borrow_mut();
        views.retain(|view| view.strong_count() != 0);
        views.is_empty()
    });
    if empty && RETIRED.with(Cell::get) {
        CONTEXT.with(|context| context.borrow_mut().take());
    }
}
