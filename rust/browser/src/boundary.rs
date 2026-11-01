//! Audited C ABI and descriptor ownership. No Rust panic unwinds into the host.
use crate::Browser;
use glib::{prelude::*, translate::*};
use gtk::{gio, prelude::*};
use std::{
    ffi::{c_char, c_void, CStr, CString},
    panic::{catch_unwind, AssertUnwindSafe},
    rc::Rc,
};
use webkit2gtk::*;
use worminal_browser_api::*;

pub(crate) struct Callback {
    event: WorminalBrowserEvent,
    context: *mut c_void,
}
impl Callback {
    pub fn emit(&self, event: u32, text: &str) {
        if let (Some(callback), Ok(text)) = (self.event, CString::new(text)) {
            unsafe {
                callback(self.context, event, text.as_ptr());
            }
        }
    }
}
fn guard<T: Default>(action: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(action)).unwrap_or_else(|_| {
        eprintln!("worminal: browser module operation panicked");
        T::default()
    })
}
unsafe fn text<'a>(pointer: *const c_char) -> &'a str {
    unsafe { CStr::from_ptr(pointer) }
        .to_str()
        .expect("UTF-8 module argument")
}
unsafe fn view<'a>(pointer: *mut c_void) -> &'a Rc<Browser> {
    unsafe { &*pointer.cast::<Rc<Browser>>() }
}
unsafe extern "C" fn profile(data: *const c_char, cache: *const c_char) -> *mut c_void {
    guard(|| {
        // gtk-rs has module-local initialization bookkeeping; GTK itself is already initialized by the host.
        gtk::init().expect("GTK browser initialization");
        let (data, cache) = unsafe { (text(data), text(cache)) };
        let manager = WebsiteDataManager::builder()
            .base_data_directory(data)
            .base_cache_directory(cache)
            .build();
        manager.set_tls_errors_policy(webkit2gtk::TLSErrorsPolicy::Fail);
        let cookies = manager.cookie_manager().expect("cookie manager");
        cookies.set_persistent_storage(
            &format!("{data}/cookies.sqlite"),
            webkit2gtk::CookiePersistentStorage::Sqlite,
        );
        cookies.set_accept_policy(webkit2gtk::CookieAcceptPolicy::NoThirdParty);
        let pointer: *mut glib::gobject_ffi::GObject =
            manager.upcast::<glib::Object>().into_glib_ptr();
        pointer.cast()
    })
}
unsafe extern "C" fn create(
    profile: *mut c_void,
    url: *const c_char,
    event: WorminalBrowserEvent,
    context: *mut c_void,
) -> *mut c_void {
    guard(|| {
        gtk::init().expect("GTK browser initialization");
        let profile: WebsiteDataManager = unsafe { from_glib_none(profile.cast()) };
        Box::into_raw(Box::new(Browser::new(
            &profile,
            unsafe { text(url) },
            Callback { event, context },
        )))
        .cast()
    })
}
unsafe extern "C" fn widget(pointer: *mut c_void) -> *mut c_void {
    guard(|| unsafe { view(pointer) }.container.as_ptr().cast())
}
unsafe extern "C" fn focus(pointer: *mut c_void) {
    guard(|| unsafe { view(pointer) }.view.grab_focus());
}
unsafe extern "C" fn navigate(pointer: *mut c_void, url: *const c_char) {
    guard(|| unsafe { view(pointer) }.view.load_uri(unsafe { text(url) }));
}
unsafe extern "C" fn shortcut(pointer: *mut c_void, key: u32, mods: u32) -> i32 {
    guard(|| i32::from(crate::chrome::shortcut(unsafe { view(pointer) }, key, mods)))
}
unsafe extern "C" fn zoom(pointer: *mut c_void, zoom: f64) {
    guard(|| {
        unsafe { view(pointer) }
            .view
            .set_zoom_level((1.0 + zoom / 10.0).max(0.25))
    });
}
unsafe extern "C" fn destroy(pointer: *mut c_void) {
    guard(|| {
        let browser = unsafe { Box::from_raw(pointer.cast::<Rc<Browser>>()) };
        unsafe {
            browser.container.destroy();
        }
        drop(browser);
        crate::release_context();
    });
}
unsafe extern "C" fn background(pointer: *mut c_void, r: f64, g: f64, b: f64, a: f64) {
    guard(|| {
        unsafe { view(pointer) }
            .view
            .set_background_color(&gtk::gdk::RGBA::new(r, g, b, a))
    });
}
unsafe extern "C" fn retire(value: u32) {
    guard(|| crate::retire(value));
}
#[no_mangle]
pub extern "C" fn worminal_rust_browser_module(abi: u32) -> *const WorminalRustBrowser {
    static API: WorminalRustBrowser = WorminalRustBrowser {
        profile: Some(profile),
        create: Some(create),
        widget: Some(widget),
        focus: Some(focus),
        navigate: Some(navigate),
        shortcut: Some(shortcut),
        zoom: Some(zoom),
        background: Some(background),
        destroy: Some(destroy),
        retire: Some(retire),
    };
    if abi == WORMINAL_RUST_BROWSER_ABI {
        &API
    } else {
        std::ptr::null()
    }
}

pub(crate) fn input(file: std::fs::File) -> gio::InputStream {
    // The stream takes the descriptor exactly once, after metadata validation on this same open file.
    unsafe { gio::UnixInputStream::take_fd(file) }.upcast()
}
