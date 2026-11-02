use crate::paint::Font;

/// One immutable pattern per window; zoom invalidates its shared metrics and description.
pub struct Fonts {
    pattern: Option<String>,
    zoom: f64,
    cached: Option<Font>,
    last_selection: i64,
}
impl Fonts {
    pub fn new(pattern: Option<String>, zoom: f64) -> Self {
        Self {
            pattern,
            zoom,
            cached: None,
            last_selection: 0,
        }
    }
    pub fn resolve(&mut self, layout: &pango::Layout) -> Font {
        let font = self
            .cached
            .get_or_insert_with(|| Font::resolve(layout, self.pattern.as_deref(), self.zoom));
        font.apply(layout);
        font.clone()
    }
    pub fn set_zoom(&mut self, zoom: f64) {
        self.zoom = zoom;
        self.cached = None;
    }
    pub fn zoom(&self) -> f64 {
        self.zoom
    }
    pub fn selection_allowance(&mut self, now: i64, interval: i64) -> bool {
        let allowed = self.last_selection == 0 || now - self.last_selection >= interval;
        self.last_selection = now;
        allowed
    }
}
