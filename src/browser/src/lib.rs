//! WebKit objects and browser policy stay inside this lazy module.
mod boundary;
mod chrome;
mod local;
mod policy;
mod profile_transfer;
mod trust;

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
    /// Downloads in flight. They live in the network process and outlive their views.
    static DOWNLOADS: Cell<usize> = const { Cell::new(0) };
}

pub(crate) struct Browser {
    container: gtk::Box,
    view: WebView,
    location: gtk::Entry,
    find: gtk::SearchEntry,
    notice: gtk::Label,
    trust_certificate: gtk::Button,
    failed_certificate: RefCell<Option<(String, gio::TlsCertificate)>>,
    select_find: Cell<bool>,
    url: RefCell<String>,
    callback: boundary::Callback,
    proxied: bool,
    profile: WebsiteDataManager,
}
impl Browser {
    fn new(
        profile: &WebsiteDataManager,
        proxied: bool,
        url: &str,
        callback: boundary::Callback,
    ) -> Rc<Self> {
        let create_context = || {
            let context = WebContext::with_website_data_manager(profile);
            if !proxied {
                local::register(&context);
            }
            context.set_spell_checking_enabled(false);
            policy::downloads(&context);
            trust::load(profile, &context);
            context
        };
        let context = if proxied {
            create_context()
        } else {
            CONTEXT.with(|slot| slot.borrow_mut().get_or_insert_with(create_context).clone())
        };
        let content = UserContentManager::new();
        let mut builder = WebView::builder().user_content_manager(&content);
        if let Some(related) = (!proxied).then(|| related(url)).flatten() {
            builder = builder.related_view(&related);
        } else {
            builder = builder.web_context(&context);
        }
        let view = builder.build();
        if !proxied {
            local::authorize(&view, &content, url);
        }
        view.set_background_color(&gtk::gdk::RGBA::new(0.094, 0.094, 0.094, 1.0));
        view.set_settings(
            &Settings::builder()
                .enable_javascript(true)
                .auto_load_images(true)
                .default_charset("UTF-8")
                .enable_developer_extras(true)
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
        let trust_certificate = gtk::Button::with_mnemonic("_Trust this certificate");
        for widget in [
            location.clone().upcast::<gtk::Widget>(),
            find.clone().upcast(),
            notice.clone().upcast(),
            trust_certificate.clone().upcast(),
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
            trust_certificate,
            failed_certificate: RefCell::new(None),
            select_find: Cell::new(false),
            url: RefCell::new(url.to_string()),
            callback,
            proxied,
            profile: profile.clone(),
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
                if browser.proxied {
                    return None;
                }
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
/// Drop the shared context once no view uses it, which ends its network process. That process otherwise keeps
/// its heap (over 100 MB after browsing) for the frontend's lifetime; the next view starts a fresh one on the
/// same profile. Pending downloads keep it alive unless retirement surrenders the profile to a new generation.
fn release_context() {
    let empty = VIEWS.with(|views| {
        let mut views = views.borrow_mut();
        views.retain(|view| view.strong_count() != 0);
        views.is_empty()
    });
    if empty && (RETIRED.with(Cell::get) || DOWNLOADS.with(Cell::get) == 0) {
        CONTEXT.with(|context| context.borrow_mut().take());
    }
}
fn download_started() {
    DOWNLOADS.with(|count| count.set(count.get() + 1));
}
fn download_finished() {
    DOWNLOADS.with(|count| count.set(count.get() - 1));
    // Release outside WebKit's signal emission, which still references the download's context.
    glib::idle_add_local_once(release_context);
}
