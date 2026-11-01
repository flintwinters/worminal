//! Audited Fontconfig/Pango-Cairo gaps in the locked GTK 3 bindings. Raw pointers never escape.
use glib::translate::ToGlibPtr;
use std::ffi::{c_char, c_double, c_int, c_void, CStr, CString};

#[link(name = "fontconfig")]
extern "C" {
    fn FcNameParse(name: *const u8) -> *mut c_void;
    fn FcPatternDestroy(pattern: *mut c_void);
    fn FcPatternGetString(
        pattern: *const c_void,
        field: *const c_char,
        index: c_int,
        value: *mut *mut u8,
    ) -> c_int;
    fn FcPatternGetDouble(
        pattern: *const c_void,
        field: *const c_char,
        index: c_int,
        value: *mut c_double,
    ) -> c_int;
}
#[link(name = "pangocairo-1.0")]
extern "C" {
    fn pango_cairo_context_set_resolution(context: *mut pango::ffi::PangoContext, dpi: c_double);
    fn pango_cairo_show_layout(
        context: *mut cairo::ffi::cairo_t,
        layout: *mut pango::ffi::PangoLayout,
    );
    fn pango_cairo_show_layout_line(
        context: *mut cairo::ffi::cairo_t,
        line: *mut pango::ffi::PangoLayoutLine,
    );
    fn pango_cairo_show_glyph_string(
        context: *mut cairo::ffi::cairo_t,
        font: *mut pango::ffi::PangoFont,
        glyphs: *mut pango::ffi::PangoGlyphString,
    );
}

pub struct FontPattern {
    pub family: Option<String>,
    pub size: Option<f64>,
    pub pixels: bool,
}
pub fn font_pattern(value: &str) -> Option<FontPattern> {
    let name = CString::new(value).ok()?;
    unsafe {
        let pattern = FcNameParse(name.as_ptr().cast());
        if pattern.is_null() {
            return None;
        }
        let mut family = std::ptr::null_mut();
        let family =
            if FcPatternGetString(pattern, b"family\0".as_ptr().cast(), 0, &mut family) == 0 {
                Some(CStr::from_ptr(family.cast()).to_string_lossy().into_owned())
            } else {
                None
            };
        let mut size = 0.0;
        let pixels = FcPatternGetDouble(pattern, b"pixelsize\0".as_ptr().cast(), 0, &mut size) == 0;
        let size = if pixels
            || FcPatternGetDouble(pattern, b"size\0".as_ptr().cast(), 0, &mut size) == 0
        {
            Some(size)
        } else {
            None
        };
        FcPatternDestroy(pattern);
        Some(FontPattern {
            family,
            size,
            pixels,
        })
    }
}
pub fn resolution(context: &pango::Context, dpi: f64) {
    unsafe { pango_cairo_context_set_resolution(context.to_glib_none().0, dpi) }
}
pub fn process_updates(window: &gtk::gdk::Window) {
    unsafe { gtk::gdk::ffi::gdk_window_process_updates(window.to_glib_none().0, 1) }
}
pub fn show_layout(context: &cairo::Context, layout: &pango::Layout) {
    unsafe { pango_cairo_show_layout(context.to_raw_none(), layout.to_glib_none().0) }
}
pub fn show_line(context: &cairo::Context, line: &pango::LayoutLine) {
    unsafe {
        let line: *mut pango::ffi::PangoLayoutLine = line.to_glib_none().0;
        // Public run offsets exist from Pango 1.50. Older libraries retain ordinary line painting.
        let runs = (*line).runs;
        if pango::version() >= 15000 && !runs.is_null() && (*runs).next.is_null() {
            let run = &*((*runs).data.cast::<pango::ffi::PangoGlyphItem>());
            let analysis = &(*run.item).analysis;
            if !analysis.font.is_null()
                && analysis.extra_attrs.is_null()
                && i32::from(analysis.gravity) == pango::ffi::PANGO_GRAVITY_SOUTH
                && i32::from(analysis.flags) & pango::ffi::PANGO_ANALYSIS_FLAG_CENTERED_BASELINE
                    == 0
                && run.y_offset == 0
                && run.start_x_offset == 0
                && run.end_x_offset == 0
            {
                pango_cairo_show_glyph_string(context.to_raw_none(), analysis.font, run.glyphs);
                return;
            }
        }
        pango_cairo_show_layout_line(context.to_raw_none(), line);
    }
}
