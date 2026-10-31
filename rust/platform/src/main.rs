//! Private-display binding compatibility check, not an application launcher.
use gtk::prelude::*;
use std::{cell::Cell, rc::Rc};
use webkit2gtk::WebViewExt;

fn main() {
    gtk::init().expect("GTK requires a private display; use manage.py check-rust-platform");
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    let view = webkit2gtk::WebView::new();
    window.add(&view);
    let loaded = Rc::new(Cell::new(false));
    let completion = loaded.clone();
    view.connect_load_changed(move |_, event| {
        if event == webkit2gtk::LoadEvent::Finished {
            completion.set(true);
        }
    });
    let context = window.create_pango_context();
    let layout = pango::Layout::new(&context);
    layout.set_text("Worminal terminal bindings");
    assert!(layout.pixel_size().0 > 0);
    let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 32, 32).unwrap();
    cairo::Context::new(&surface).unwrap().paint().unwrap();
    window.show_all();
    view.load_html("<!doctype html><title>Worminal binding check</title>", None);
    // WebKit publishes the title separately from load completion; require both milestones.
    while !loaded.get() || view.title().as_deref() != Some("Worminal binding check") {
        glib::MainContext::default().iteration(true);
    }
    window.close();
    println!("Rust GTK/Pango/Cairo/WebKit bindings: OK");
}
