//! Application chrome shares terminal metrics, while labels keep filesystem bytes out of display text handling.
use super::{App, LINK_SHIFT};
use crate::tabs::Tab;
use gtk::{gio, prelude::*};
use std::rc::Rc;
use worminal_protocol::worp::commands::CommandKind;

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
        let entries: Vec<_> = pages
            .iter()
            .map(|page| {
                let label = if page.browser {
                    browser_label(&page.url, &page.title)
                } else {
                    use std::os::unix::ffi::OsStrExt;
                    String::from_utf8_lossy(&worminal_protocol::worp::events::tab_label(
                        page.directory.as_os_str().as_bytes(),
                        page.ssh_user.as_deref().map(str::as_bytes),
                    ))
                    .into_owned()
                };
                page.label.set_text(&label);
                Tab {
                    id: page.id,
                    label,
                    browser: page.browser,
                    remote: self.master_of(page.id).is_some(),
                    ssh: page.ssh_user.is_some(),
                }
            })
            .collect();
        for pane in self.panes() {
            let members = self.tabs_of(&pane);
            let active = self.active_of(&pane).unwrap_or(0);
            let palette = pages
                .iter()
                .find(|page| page.id == active)
                .map(|page| page.palette.clone());
            let own = entries
                .iter()
                .filter(|entry| members.contains(&entry.id))
                .cloned()
                .collect();
            pane.strip.update(own, active, palette);
        }
    }
    pub(super) fn select_index(self: &Rc<Self>, index: u32) {
        let notebook = self.notebook();
        let widget = notebook.nth_page(Some(index));
        let id = self
            .pages
            .borrow()
            .iter()
            .find(|page| Some(&page.widget) == widget.as_ref())
            .map(|page| page.id);
        if let Some(id) = id {
            self.select_id(id);
        }
    }
    /// The daemon owns tab order. Ask it to put `id` in `target`'s slot; the catalog it publishes reorders the strip.
    /// Tabs of different daemons keep their link grouping, so those drops are ignored.
    pub(super) fn move_tab(self: &Rc<Self>, id: u32, target: u32) {
        let link = id >> LINK_SHIFT;
        if target >> LINK_SHIFT != link {
            return;
        }
        let position = self
            .pages
            .borrow()
            .iter()
            .filter(|page| page.id >> LINK_SHIFT == link)
            .position(|page| page.id == target);
        if let Some(position) = position {
            self.send(
                id,
                CommandKind::Move {
                    position: position as u32,
                },
            );
        }
    }
    pub(super) fn select_id(self: &Rc<Self>, id: u32) {
        let Some(pane) = self.pane_of(id) else { return };
        let index = self
            .page_widget(id)
            .and_then(|widget| pane.notebook.page_num(&widget));
        self.focus_pane(&pane);
        if pane.notebook.current_page() == index {
            self.activate(id);
        } else {
            pane.notebook.set_current_page(index);
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
