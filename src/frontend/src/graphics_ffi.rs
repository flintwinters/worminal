//! Fontconfig/Pango/Cairo calls missing from the Rust GTK bindings. Raw pointers stay in this module.
use glib::translate::ToGlibPtr;
use std::ffi::{c_char, c_double, c_int, c_void, CStr, CString};

#[cfg(worminal_gdk_x11)]
#[link(name = "gdk-3")]
extern "C" {
    fn gdk_x11_display_get_type() -> glib::ffi::GType;
    fn gdk_x11_get_server_time(window: *mut gtk::gdk::ffi::GdkWindow) -> u32;
    fn gdk_x11_window_set_user_time(window: *mut gtk::gdk::ffi::GdkWindow, time: u32);
}

pub(crate) fn show_window(window: &gtk::Window) {
    use gtk::prelude::*;
    #[allow(unused_mut)]
    let mut time = gtk::gdk::ffi::GDK_CURRENT_TIME as u32;
    #[cfg(worminal_gdk_x11)]
    {
        let x11: glib::Type = unsafe { glib::translate::from_glib(gdk_x11_display_get_type()) };
        if window.display().type_().is_a(x11) {
            // Stamp mapping and presentation for focus policies that reject an owner's stale input time.
            // The private Muffin regression also checks the property: permissive policies can mask this bug.
            window.realize();
            if let Some(surface) = window.window() {
                unsafe {
                    time = gdk_x11_get_server_time(surface.to_glib_none().0);
                    gdk_x11_window_set_user_time(surface.to_glib_none().0, time);
                }
            }
        }
    }
    window.show_all();
    window.present_with_time(time);
}

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
        let family = if FcPatternGetString(pattern, c"family".as_ptr().cast(), 0, &mut family) == 0
        {
            Some(CStr::from_ptr(family.cast()).to_string_lossy().into_owned())
        } else {
            None
        };
        let mut size = 0.0;
        let pixels = FcPatternGetDouble(pattern, c"pixelsize".as_ptr().cast(), 0, &mut size) == 0;
        let size =
            if pixels || FcPatternGetDouble(pattern, c"size".as_ptr().cast(), 0, &mut size) == 0 {
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
