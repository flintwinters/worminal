//! Application chrome shares terminal metrics, while labels keep filesystem bytes out of display text handling.
use super::App;
use crate::tabs::Tab;
use gtk::{gio, prelude::*};
use std::rc::Rc;

fn browser_label(url: &str, title: &str) -> String {
    if let Ok(uri) = glib::Uri::parse(url, glib::UriFlags::ENCODED) {
        if uri.scheme().eq_ignore_ascii_case("file") {
            let document =
                uri.to_string_partial(glib::UriHideFlags::QUERY | glib::UriHideFlags::FRAGMENT);
            if let Some(name) = gio::File::for_uri(&document).basename() {
                return name.to_string_lossy().into_owned();
            }
        }
    }
    title.to_string()
}
impl App {
    pub(super) fn refresh_tabs(&self) {
        if self.syncing.get() {
            return;
        }
        let pages = self.pages.borrow();
        let entries = pages
            .iter()
            .map(|page| {
                let label = if page.browser {
                    browser_label(&page.url, &page.title)
                } else {
                    page.directory
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| page.directory.to_string_lossy().into_owned())
                };
                page.label.set_text(&label);
                Tab {
                    id: page.id,
                    label,
                    browser: page.browser,
                    remote: self.master_of(page.id).is_some(),
                }
            })
            .collect();
        let palette = pages
            .iter()
            .find(|page| page.id == self.active.get())
            .map(|page| page.palette.clone());
        self.tabstrip.update(entries, self.active.get(), palette);
    }
    pub(super) fn select_index(self: &Rc<Self>, index: u32) {
        if self.notebook.current_page() == Some(index) {
            let id = self.notebook.nth_page(Some(index)).and_then(|widget| {
                self.pages
                    .borrow()
                    .iter()
                    .find(|page| page.widget == widget)
                    .map(|page| page.id)
            });
            if let Some(id) = id {
                self.activate(id);
            }
        } else {
            self.notebook.set_current_page(Some(index));
        }
    }
    pub(super) fn select_id(self: &Rc<Self>, id: u32) {
        let index = self
            .pages
            .borrow()
            .iter()
            .find(|page| page.id == id)
            .and_then(|page| self.notebook.page_num(&page.widget));
        if let Some(index) = index {
            self.select_index(index);
        }
    }
}
pub(super) fn outline(window: &gtk::Window, content: &gtk::Box) {
    let overlay = gtk::Overlay::new();
    overlay.add(content);
    let border = gtk::DrawingArea::new();
    // A windowed overlay would intercept wheel events above terminal/WebKit content.
    border.set_has_window(false);
    overlay.add_overlay(&border);
    overlay.set_overlay_pass_through(&border, true);
    border.connect_draw(glib::clone!(@weak window => @default-return glib::Propagation::Proceed, move |widget, cr| {
        let shade = if window.is_active() { 0.6 } else { 0.3 };
        cr.set_source_rgb(shade, shade, shade); cr.set_line_width(1.0);
        cr.rectangle(0.5, 0.5, f64::from(widget.allocated_width() - 1), f64::from(widget.allocated_height() - 1));
        if let Err(error) = cr.stroke() { eprintln!("worminal: outline: {error}"); }
        glib::Propagation::Proceed
    }));
    window.connect_is_active_notify(glib::clone!(@weak border => move |_| {
        let width = border.allocated_width(); let height = border.allocated_height();
        let edges = [(0, 0, width, 1), (0, height - 1, width, 1),
            (0, 0, 1, height), (width - 1, 0, 1, height)];
        for (x, y, w, h) in edges {
            border.queue_draw_area(x, y, w, h);
        }
    }));
    window.add(&overlay);
}

#[cfg(test)]
mod tests {
    use super::browser_label;
    #[test]
    fn file_labels_decode_the_path_without_query_or_fragment() {
        assert_eq!(
            browser_label(
                "file:///a/literal%20%2B%23%3F%25%20%C3%BC.html?route=x#anchor",
                "Page title"
            ),
            "literal +#?% ü.html"
        );
        assert_eq!(
            browser_label("https://example.invalid/a.html?route=x", "Document title"),
            "Document title"
        );
    }
}
