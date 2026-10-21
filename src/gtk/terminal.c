/* GTK owns surfaces and input; st owns cells and selection, the service owns PTYs. */
#include <math.h>
#include <stdlib.h>
#include <string.h>
#include <wchar.h>
#include <pango/pangocairo.h>
#include <fontconfig/fontconfig.h>
#include "preview.h"
#include "win.h"
#include WORMINAL_THEME_HEADER

struct Terminal {
	Page *page;
	TermSession *session;
	SessionFrame frame;
	GtkIMContext *im;
	PangoFontDescription *font;
	PangoLayout *layout;
	PangoLayout *ascii[4][95];
	char *preedit;
	int cw, ch, height, ascent, cols, rows;
	int button, last_x, last_y;
	gboolean selecting, blink, numlock;
	gboolean full_damage, input_pending, selection_pending;
	guint8 damaged_rows[200];
	guint timer;
};

static void damage_row(Terminal *, int);
static void prepare(Terminal *);

void
terminal_select(Terminal *t)
{
	App *app = t->page->app;
	gint64 now = g_get_monotonic_time(), interval = 16667, presentation;
	GdkFrameClock *clock = gtk_widget_get_frame_clock(t->page->widget);
	if (clock) gdk_frame_clock_get_refresh_info(clock, now, &interval, &presentation);
	/* Cached tab mapping already schedules a draw. Only a new service frame
	 * for ownership or geometry needs a separate response allowance. Rapid
	 * selections retain GTK's batching after the first response in a burst. */
	if (t->page->live && t->frame.cols == t->cols && t->frame.rows == t->rows)
		t->selection_pending = FALSE;
	else if (!app->last_selection || now - app->last_selection >= interval)
		t->selection_pending = TRUE;
	app->last_selection = now;
}

static void
scroll_view(Terminal *t, gboolean up, int steps)
{
	WireBuffer b = {0};
	wire_put_u32(&b, up); wire_put_u32(&b, steps);
	app_send(t->page->app, WIRE_SCROLL, t->page->id, b.data, b.len);
	wire_buffer_free(&b);
}

static void
input(Terminal *t, const char *bytes, size_t length)
{
	if (t->page->mode & MODE_KBDLOCK)
		return;
	prepare(t);
	app_startuptime("input");
	if (!t->page->live) app_focus(t->page);
	tsessionuse(t->session);
	tsessioncleandirty(t->session);
	selclear();
	for (int row = 0; row < t->frame.rows; row++)
		if (tsessionrowdirty(t->session, row)) damage_row(t, row);
	app_send(t->page->app, WIRE_INPUT, t->page->id, bytes, length);
	if (length) t->input_pending = TRUE;
}

static void
commit(GtkIMContext *im, const char *text, Terminal *t)
{
	(void)im;
	input(t, text, strlen(text));
}

static void
preedit(GtkIMContext *im, Terminal *t)
{
	PangoAttrList *attrs;
	int cursor;
	g_free(t->preedit);
	gtk_im_context_get_preedit_string(im, &t->preedit, &attrs, &cursor);
	pango_attr_list_unref(attrs);
	gtk_widget_queue_draw(t->page->widget);
}

static void
resize(GtkWidget *widget, GtkAllocation *allocation, Terminal *t)
{
	(void)widget;
	if (!t->layout) return;
	App *app = t->page->app;
	if (app->geometry_pending && app->active == t->page && gtk_widget_get_mapped(app->window)) {
		int width, height;
		app->geometry_pending = FALSE;
		gtk_window_get_size(GTK_WINDOW(app->window), &width, &height);
		gtk_window_resize(GTK_WINDOW(app->window), app->cols * t->cw + width - allocation->width + 4,
		                  app->rows * t->ch + height - allocation->height + 4);
	}
	t->cols = CLAMP((allocation->width - 4) / MAX(1, t->cw), 1, 400);
	t->rows = CLAMP((allocation->height - 4) / MAX(1, t->ch), 1, 200);
	if (t->page->app->active == t->page &&
	    (t->page->live || gtk_window_is_active(GTK_WINDOW(t->page->app->window))))
		app_resize(t->page);
}

static void
clear_layouts(Terminal *t)
{
	for (size_t style = 0; style < G_N_ELEMENTS(t->ascii); style++)
		for (size_t c = 0; c < G_N_ELEMENTS(t->ascii[style]); c++)
			g_clear_object(&t->ascii[style][c]);
}

PangoFontDescription *
terminal_font(App *app, PangoLayout *layout, int *cw, int *ch, int *height, int *ascent)
{
	/* Pango's Fontconfig backend selects FreeType fonts and fallback faces.
	 * Point sizes follow the existing Xft renderer's default 96 dpi. */
	PangoContext *context = pango_layout_get_context(layout);
	pango_cairo_context_set_resolution(context, 96);
	if (app->cached_font && app->cached_font_zoom == app->zoom) {
		pango_layout_set_font_description(layout, app->cached_font);
		*cw = app->font_cw; *ch = app->font_ch;
		*height = app->font_height; *ascent = app->font_ascent;
		return pango_font_description_copy(app->cached_font);
	}
	PangoFontDescription *font = pango_font_description_new();
	double size = theme_font_size;
	gboolean pixels = FALSE;
	const char *family = theme_font_family;
	FcPattern *pattern = app->font ? FcNameParse((const FcChar8 *)app->font) : NULL;
	if (pattern) {
		FcChar8 *matched;
		if (FcPatternGetString(pattern, FC_FAMILY, 0, &matched) == FcResultMatch) family = (char *)matched;
		pixels = FcPatternGetDouble(pattern, FC_PIXEL_SIZE, 0, &size) == FcResultMatch;
		if (!pixels) FcPatternGetDouble(pattern, FC_SIZE, 0, &size);
	}
	pango_font_description_set_family(font, family);
	if (pattern) FcPatternDestroy(pattern);
	if (pixels) pango_font_description_set_absolute_size(font, MAX(4, size + app->zoom) * PANGO_SCALE);
	else pango_font_description_set_size(font, MAX(4, size + app->zoom) * PANGO_SCALE);
	pango_layout_set_font_description(layout, font);
	pango_layout_set_text(layout,
	    " !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~", -1);
	int width;
	pango_layout_get_size(layout, &width, NULL);
	*cw = MAX(1, (int)ceil(width / (95.0 * PANGO_SCALE)));
	PangoFontMetrics *metrics = pango_context_get_metrics(context, font, pango_context_get_language(context));
	*ascent = PANGO_PIXELS_CEIL(pango_font_metrics_get_ascent(metrics));
	*height = *ascent + PANGO_PIXELS_CEIL(pango_font_metrics_get_descent(metrics));
	*ch = MAX(1, *height + theme_font_offset_y);
	pango_font_metrics_unref(metrics);
	g_clear_pointer(&app->cached_font, pango_font_description_free);
	app->cached_font = pango_font_description_copy(font);
	app->cached_font_zoom = app->zoom;
	app->font_cw = *cw; app->font_ch = *ch;
	app->font_height = *height; app->font_ascent = *ascent;
	return font;
}

void
terminal_zoom(Terminal *t)
{
	if (!t->layout) { prepare(t); return; }
	clear_layouts(t);
	if (t->font) pango_font_description_free(t->font);
	t->font = terminal_font(t->page->app, t->layout, &t->cw, &t->ch, &t->height, &t->ascent);
	GtkAllocation allocation;
	/* A new widget still has GTK's placeholder 1x1 allocation. Keep the
	 * requested PTY geometry until GTK has actually laid the terminal out. */
	if (gtk_widget_get_realized(t->page->widget)) {
		gtk_widget_get_allocation(t->page->widget, &allocation);
		resize(t->page->widget, &allocation, t);
	}
	gtk_widget_queue_draw(t->page->widget);
}

gboolean
terminal_palette(Page *p, uint32_t index, const char *name)
{
	if (index >= G_N_ELEMENTS(p->colors))
		return FALSE;
	GdkRGBA color = p->colors[index];
	if (name && gdk_rgba_parse(&color, name)) {
		/* The service's per-tab palette overrides the compiled defaults. */
	} else if (index >= 16 && index < 256) {
		unsigned int n = index - 16;
		if (index < 232) {
			unsigned int rgb[] = {n / 36, n / 6 % 6, n % 6};
			color = (GdkRGBA){rgb[0] ? (55 + 40 * rgb[0]) / 255.0 : 0,
			                  rgb[1] ? (55 + 40 * rgb[1]) / 255.0 : 0,
			                  rgb[2] ? (55 + 40 * rgb[2]) / 255.0 : 0, 1};
		} else {
			double gray = (8 + 10 * (index - 232)) / 255.0;
			color = (GdkRGBA){gray, gray, gray, 1};
		}
	} else if (index < G_N_ELEMENTS(colorname) && colorname[index]) {
		gdk_rgba_parse(&color, colorname[index]);
	}
	gboolean changed = !gdk_rgba_equal(&p->colors[index], &color);
	p->colors[index] = color;
	return changed;
}

static GdkRGBA
color(Page *p, uint32_t value)
{
	if (IS_TRUECOL(value))
		return (GdkRGBA){((value >> 16) & 255) / 255.0,
		                 ((value >> 8) & 255) / 255.0, (value & 255) / 255.0, 1};
	return p->colors[value < G_N_ELEMENTS(p->colors) ? value : defaultfg];
}

static gboolean
cell_colors(Terminal *t, Glyph g, int x, int y, GdkRGBA *fg, GdkRGBA *bg)
{
	Page *p = t->page;
	if (theme_bold_bright && (g.mode & ATTR_BOLD) && g.fg < 8)
		g.fg += 8;
	*fg = color(p, g.fg);
	*bg = color(p, g.bg);
	gboolean reverse = !!(g.mode & ATTR_REVERSE) ^ !!(p->mode & MODE_REVERSE);
	if (selected(x, y))
		reverse = !reverse;
	if (reverse) {
		GdkRGBA swap = *fg;
		*fg = *bg;
		*bg = swap;
	}
	if (g.mode & ATTR_FAINT) {
		fg->red *= .66; fg->green *= .66; fg->blue *= .66;
	}
	if ((g.mode & ATTR_INVISIBLE) || ((g.mode & ATTR_BLINK) && t->blink))
		*fg = *bg;
	SessionFrame f = t->frame;
	gboolean cursor_cell = x == f.cursor_x || (x == f.cursor_x + 1 &&
	    (tsessionviewline(t->session, y)[f.cursor_x].mode & ATTR_WIDE));
	gboolean cursor = !f.scroll && cursor_cell && y == f.cursor_y &&
	    !(p->mode & MODE_HIDE) && (!t->blink || !(p->cursor & 1)) && p->live;
	gboolean block = cursor && p->cursor <= 2 && gtk_widget_has_focus(p->widget);
	if (block) { *bg = color(p, defaultcs); *fg = color(p, defaultbg); }
	return cursor && !block;
}

static PangoGlyphItem *
plain_ascii_run(PangoLayoutLine *line)
{
#if PANGO_VERSION_CHECK(1, 50, 0)
	/* Run offsets became public in 1.50. Older libraries keep line drawing. */
	PangoGlyphItem *run = line->runs && !line->runs->next ? line->runs->data : NULL;
	return run && run->item->analysis.font && !run->item->analysis.extra_attrs &&
	    run->item->analysis.gravity == PANGO_GRAVITY_SOUTH &&
	    !(run->item->analysis.flags & PANGO_ANALYSIS_FLAG_CENTERED_BASELINE) &&
	    !run->y_offset && !run->start_x_offset && !run->end_x_offset ? run : NULL;
#else
	(void)line;
	return NULL;
#endif
}

static void
text(Terminal *t, cairo_t *cr, const char *value, int x, int y, ushort mode)
{
	/* ASCII shaping repeats across almost every cell and frame. Keep a bounded
	 * cache by glyph and font style; colors and decorations do not affect shape.
	 * Unicode/preedit still use Pango fallback without an unbounded text cache. */
	unsigned int style = !!(mode & ATTR_BOLD) | (!!(mode & ATTR_ITALIC) << 1);
	PangoLayout **cached = value[0] >= ' ' && value[0] <= '~' && !value[1]
	    ? &t->ascii[style][value[0] - ' '] : NULL;
	PangoLayout *layout = cached ? *cached : t->layout;
	if (!cached || !layout) {
		if (cached) layout = *cached = gtk_widget_create_pango_layout(t->page->widget, "");
		pango_font_description_set_weight(t->font, mode & ATTR_BOLD ? PANGO_WEIGHT_BOLD : PANGO_WEIGHT_NORMAL);
		pango_font_description_set_style(t->font, mode & ATTR_ITALIC ? PANGO_STYLE_ITALIC : PANGO_STYLE_NORMAL);
		pango_layout_set_font_description(layout, t->font);
		pango_layout_set_text(layout, value, -1);
	}
	/* Anchor glyphs to cell bottoms, including compact spacing and glyph offset.
	 * Draw every background first: tall glyphs may overlap adjacent compact rows. */
	double top = 2 + (y + 1) * t->ch - t->height - theme_glyph_offset_y;
	if (cached) {
		/* Keep the shaped line baseline and font; plain horizontal runs can
		 * bypass layout rendering. Attributed/offset runs keep line drawing. */
		cairo_move_to(cr, 2 + x * t->cw, top + pango_layout_get_baseline(layout) / (double)PANGO_SCALE);
		PangoLayoutLine *line = pango_layout_get_line_readonly(layout, 0);
		PangoGlyphItem *run = plain_ascii_run(line);
		if (run) pango_cairo_show_glyph_string(cr, run->item->analysis.font, run->glyphs);
		else pango_cairo_show_layout_line(cr, line);
	} else {
		cairo_move_to(cr, 2 + x * t->cw, top);
		pango_cairo_show_layout(cr, layout);
	}
}

static int
glyph_overlap(Terminal *t)
{
	return t->height + abs(theme_glyph_offset_y);
}

static void
background_run(Terminal *t, cairo_t *cr, const GdkRGBA *bg, int first, int end, int row)
{
	gdk_cairo_set_source_rgba(cr, bg);
	cairo_rectangle(cr, 2 + first * t->cw, 2 + row * t->ch, (end - first) * t->cw, t->ch);
	cairo_fill(cr);
}

static void
draw_backgrounds(Terminal *t, cairo_t *cr, int first, int end)
{
	cairo_matrix_t matrix;
	cairo_get_matrix(cr, &matrix);
	/* Joining cells is pixel-identical only on an aligned device grid. Keep
	 * scalar painting for fractional transforms, including alpha edge blends. */
	double origin_x = 2 * matrix.xx + matrix.x0, origin_y = 2 * matrix.yy + matrix.y0;
	double step_x = t->cw * matrix.xx, step_y = t->ch * matrix.yy;
	gboolean aligned = !matrix.xy && !matrix.yx &&
	    origin_x == floor(origin_x) && origin_y == floor(origin_y) &&
	    step_x == floor(step_x) && step_y == floor(step_y);
	for (int y = first; y < end; y++) {
		Line line = tsessionviewline(t->session, y);
		GdkRGBA fg, bg, run;
		cell_colors(t, line[0], 0, y, &fg, &run);
		int start = 0;
		for (int x = 1; x < t->frame.cols; x++) {
			cell_colors(t, line[x], x, y, &fg, &bg);
			if (aligned && gdk_rgba_equal(&bg, &run)) continue;
			background_run(t, cr, &run, start, x, y);
			start = x;
			run = bg;
		}
		background_run(t, cr, &run, start, t->frame.cols, y);
	}
}

static gboolean
draw_terminal(GtkWidget *widget, cairo_t *cr, Terminal *t)
{
	prepare(t);
	app_startuptime("paint");
	Page *p = t->page;
	tsessionuse(t->session);
	GdkRGBA base = color(p, p->mode & MODE_REVERSE ? defaultfg : defaultbg);
	gdk_cairo_set_source_rgba(cr, &base);
	cairo_paint(cr);
	SessionFrame f = t->frame;
	double clip_x1, clip_y1, clip_x2, clip_y2;
	cairo_clip_extents(cr, &clip_x1, &clip_y1, &clip_x2, &clip_y2);
	/* Compact glyphs can extend beyond their row. Include every source row
	 * whose background or glyph ink can intersect the damaged area. */
	int overlap = glyph_overlap(t) + t->ch;
	int first = MAX(0, (int)floor((clip_y1 - 2 - overlap) / t->ch));
	int end = MIN(f.rows, (int)ceil((clip_y2 - 2 + overlap) / t->ch));
	draw_backgrounds(t, cr, first, end);
	for (int y = first; y < end; y++) {
		Line line = tsessionviewline(t->session, y);
		for (int x = 0; x < f.cols; x++) {
			Glyph g = line[x];
			if (g.mode & ATTR_WDUMMY) continue;
			gboolean blank = g.u == ' ' && !(g.mode & (ATTR_UNDERLINE | ATTR_STRUCK));
			/* Plain spaces have no foreground ink. Only the cursor row can
			 * still require an outline or another non-block cursor shape. */
			if (blank && y != f.cursor_y) continue;
			GdkRGBA fg, bg;
			gboolean cursor = cell_colors(t, g, x, y, &fg, &bg);
			if (blank && !cursor) continue;
			char utf8[8] = {0};
			g_unichar_to_utf8(g_unichar_validate(g.u) ? g.u : 0xfffd, utf8);
			gdk_cairo_set_source_rgba(cr, &fg);
			if (g.u != ' ') text(t, cr, utf8, x, y, g.mode);
			int width = t->cw * ((g.mode & ATTR_WIDE) ? 2 : 1);
			if (g.mode & ATTR_UNDERLINE) {
				cairo_rectangle(cr, 2 + x * t->cw, 2 + (y + 1) * t->ch - 1, width, 1);
				cairo_fill(cr);
			}
			if (g.mode & ATTR_STRUCK) {
				cairo_rectangle(cr, 2 + x * t->cw, 2 + y * t->ch + t->ch / 2, width, 1);
				cairo_fill(cr);
			}
			if (cursor) {
				GdkRGBA cs = color(p, defaultcs);
				gdk_cairo_set_source_rgba(cr, &cs);
				if (p->cursor <= 2) {
					cairo_set_line_width(cr, 1);
					cairo_rectangle(cr, 2.5 + x * t->cw, 2.5 + y * t->ch, width - 1, t->ch - 1);
					cairo_stroke(cr);
				} else if (p->cursor == 7) {
					text(t, cr, "☃", x, y, 0);
				} else if (p->cursor == 5 || p->cursor == 6)
					cairo_rectangle(cr, 2 + x * t->cw, 2 + y * t->ch, 2, t->ch);
				else
					cairo_rectangle(cr, 2 + x * t->cw, (y + 1) * t->ch, t->cw, 2);
				cairo_fill(cr);
			}
		}
	}
	if (t->preedit && *t->preedit) {
		GdkRGBA fg = color(p, defaultfg);
		gdk_cairo_set_source_rgba(cr, &fg);
		text(t, cr, t->preedit, f.cursor_x, f.cursor_y, ATTR_UNDERLINE);
	}
	GdkRectangle spot = {2 + f.cursor_x * t->cw, 2 + f.cursor_y * t->ch, t->cw, t->ch};
	gtk_im_context_set_cursor_location(t->im, &spot);
	return FALSE;
}

static void
damage_row(Terminal *t, int row)
{
	/* Include ink spilling into adjacent rows, then let draw_terminal restore
	 * their backgrounds and overlapping glyphs in the usual two passes. */
	int overlap = glyph_overlap(t);
	gtk_widget_queue_draw_area(t->page->widget, 0, 2 + row * t->ch - overlap,
	    gtk_widget_get_allocated_width(t->page->widget), t->ch + 2 * overlap);
}

static gboolean
blink(gpointer data)
{
	Terminal *t = data;
	t->blink = !t->blink;
	if (!t->session || !gtk_widget_get_mapped(t->page->widget)) return G_SOURCE_CONTINUE;
	Page *p = t->page;
	for (int row = 0; row < t->frame.rows; row++) {
		gboolean changed = row == t->frame.cursor_y && !t->frame.scroll &&
		    p->live && !(p->mode & MODE_HIDE) && (p->cursor & 1);
		Line line = tsessionviewline(t->session, row);
		for (int col = 0; col < t->frame.cols && !changed; col++)
			changed = line[col].mode & ATTR_BLINK;
		if (changed) damage_row(t, row);
	}
	return G_SOURCE_CONTINUE;
}

void
terminal_configure(Terminal *t, uint32_t mode, uint32_t cursor, gboolean palette_changed)
{
	Page *p = t->page;
	uint32_t changed_mode = p->mode ^ mode;
	gboolean changed_cursor = p->cursor != cursor || (changed_mode & MODE_HIDE);
	p->mode = mode; p->cursor = cursor;
	/* Catalog-only style updates need damage of their own. Idle blink ticks
	 * cannot be relied on to eventually repaint unrelated state changes. */
	if (palette_changed && p->app->active == p && p->app->tabstrip)
		gtk_widget_queue_draw(p->app->tabstrip);
	if (!t->layout || !gtk_widget_get_mapped(p->widget)) return;
	if (palette_changed || (changed_mode & MODE_REVERSE)) gtk_widget_queue_draw(p->widget);
	else if (changed_cursor) damage_row(t, t->frame.cursor_y);
}

static gboolean
focus(GtkWidget *widget, GdkEventFocus *event, Terminal *t)
{
	prepare(t);
	gtk_im_context_set_client_window(t->im, gtk_widget_get_window(widget));
	if (event->in) {
		gtk_im_context_focus_in(t->im);
		app_focus(t->page);
	} else {
		gtk_im_context_focus_out(t->im);
	}
	if (t->page->mode & MODE_FOCUS) {
		WireBuffer b = {0};
		wire_put_u32(&b, event->in);
		app_send(t->page->app, WIRE_FOCUS_STATE, t->page->id, b.data, b.len);
		wire_buffer_free(&b);
	}
	gtk_widget_queue_draw(widget);
	return FALSE;
}

void
terminal_dimensions(Terminal *t, int *cols, int *rows)
{
	*cols = t->cols; *rows = t->rows;
}

void
terminal_copy(Terminal *t, GdkAtom selection)
{
	if (!t->session) return;
	tsessionuse(t->session);
	char *value = getsel();
	if (value) {
		gtk_clipboard_set_text(gtk_clipboard_get(selection), value, -1);
		free(value);
	}
}

static void
pasted(GtkClipboard *clipboard, const char *value, gpointer data)
{
	(void)clipboard;
	/* Hold the widget while the clipboard request is pending; destruction marks
	 * it unusable, so an asynchronous reply cannot dereference a freed terminal. */
	GtkWidget *widget = data;
	Terminal *t = g_object_get_data(G_OBJECT(widget), "terminal");
	if (t && value) {
		char *clean = g_strdup(value);
		for (char *p = clean; *p; p++)
			if (*p == '\n') *p = '\r';
		gboolean bracket = t->page->mode & MODE_BRCKTPASTE;
		if (bracket) input(t, "\033[200~", 6);
		/* Bound each protocol packet. The transport applies its own queue limit. */
		for (size_t offset = 0, length = strlen(clean); offset < length; offset += 32768)
			input(t, clean + offset, MIN(32768, length - offset));
		if (bracket) input(t, "\033[201~", 6);
		g_free(clean);
	}
	g_object_unref(widget);
}

void
terminal_paste(Terminal *t, GdkAtom selection)
{
	gtk_clipboard_request_text(gtk_clipboard_get(selection), pasted, g_object_ref(t->page->widget));
}

typedef struct { guint key, mask; const char *text; int appkey, appcursor; } Key;
#define KEY(name) GDK_KEY_##name
#define KEY_ANY_MOD G_MAXUINT
#define KEY_NO_MOD 0
#define KEY_SHIFT GDK_SHIFT_MASK
#define KEY_CTRL GDK_CONTROL_MASK
#define KEY_ALT GDK_MOD1_MASK
#define KEY_SUPER GDK_MOD4_MASK
#define KEY_MOD3 GDK_MOD3_MASK
static const Key keys[] = {
#include "keymap.inc"
};

static const char *
keymap(Terminal *t, GdkEventKey *e)
{
	guint state = e->state & (GDK_SHIFT_MASK | GDK_CONTROL_MASK | GDK_MOD1_MASK | GDK_MOD4_MASK | GDK_MOD3_MASK);
	for (size_t i = 0; i < G_N_ELEMENTS(keys); i++) {
		const Key *k = &keys[i];
		if (k->key != e->keyval || (k->mask != G_MAXUINT && k->mask != state)) continue;
		if ((t->page->mode & MODE_APPKEYPAD) ? k->appkey < 0 : k->appkey > 0) continue;
		if (t->numlock && k->appkey == 2) continue;
		if ((t->page->mode & MODE_APPCURSOR) ? k->appcursor < 0 : k->appcursor > 0) continue;
		return k->text;
	}
	return NULL;
}

gboolean
terminal_key(Terminal *t, GdkEventKey *e)
{
	prepare(t);
	if (e->type == GDK_KEY_RELEASE)
		return gtk_im_context_filter_keypress(t->im, e);
	if ((e->state & (GDK_CONTROL_MASK | GDK_SHIFT_MASK)) == (GDK_CONTROL_MASK | GDK_SHIFT_MASK) &&
	    e->keyval == GDK_KEY_Num_Lock) { t->numlock = !t->numlock; return TRUE; }
	if ((e->state & GDK_SHIFT_MASK) && e->keyval == GDK_KEY_Insert) {
		terminal_paste(t, GDK_SELECTION_PRIMARY);
		return TRUE;
	}
	if (e->keyval == GDK_KEY_Print || e->keyval == GDK_KEY_Break) {
		if (e->keyval == GDK_KEY_Print && !(e->state & (GDK_CONTROL_MASK | GDK_SHIFT_MASK))) {
			tsessionuse(t->session); printsel(NULL);
		} else {
			WireBuffer b = {0};
			wire_put_u32(&b, e->keyval == GDK_KEY_Break ? 2 : (e->state & GDK_CONTROL_MASK ? 0 : 1));
			app_send(t->page->app, WIRE_CONTROL, t->page->id, b.data, b.len);
			wire_buffer_free(&b);
		}
		return TRUE;
	}
	if ((e->state & GDK_SHIFT_MASK) && (e->keyval == GDK_KEY_Page_Up || e->keyval == GDK_KEY_Page_Down)) {
		scroll_view(t, e->keyval == GDK_KEY_Page_Up, MAX(1, t->frame.rows - 1));
		return TRUE;
	}
	const char *mapped = keymap(t, e);
	if (mapped) {
		input(t, mapped, strlen(mapped));
		return TRUE;
	}
	if (e->state & GDK_CONTROL_MASK) {
		gunichar c = gdk_keyval_to_unicode(e->keyval);
		if ((c >= '@' && c <= '_') || (c >= 'a' && c <= 'z') || c == ' ') {
			char byte = c == ' ' ? 0 : c & 31;
			if (e->state & GDK_MOD1_MASK) input(t, "\033", 1);
			input(t, &byte, 1);
			return TRUE;
		}
	}
	gunichar alt = gdk_keyval_to_unicode(e->keyval);
	if ((e->state & GDK_MOD1_MASK) && alt >= 32 && alt < 127) {
		char bytes[8] = {0};
		if (t->page->mode & MODE_8BIT) alt |= 0x80;
		else input(t, "\033", 1);
		input(t, bytes, g_unichar_to_utf8(alt, bytes));
		return TRUE;
	}
	if (gtk_im_context_filter_keypress(t->im, e))
		return TRUE;
	gunichar c = gdk_keyval_to_unicode(e->keyval);
	if (c) {
		char bytes[8] = {0};
		int length = g_unichar_to_utf8(c, bytes);
		input(t, bytes, length);
		return TRUE;
	}
	return FALSE;
}

static gboolean
key_event(GtkWidget *widget, GdkEventKey *event, Terminal *t)
{
	(void)widget;
	return terminal_key(t, event);
}

static void
mouse_position(Terminal *t, double px, double py, int *x, int *y)
{
	*x = CLAMP((int)(px - 2) / t->cw, 0, t->frame.cols - 1);
	*y = CLAMP((int)(py - 2) / t->ch, 0, t->frame.rows - 1);
}

static void
mouse_report(Terminal *t, int button, int x, int y, guint state, gboolean release, gboolean motion)
{
	uint32_t mode = t->page->mode;
	if ((mode & MODE_MOUSEX10) && (release || motion)) return;
	if (motion && !(mode & MODE_MOUSEMANY) && !(t->button && (mode & MODE_MOUSEMOTION))) return;
	int code = button + (motion ? 32 : 0);
	if (!(mode & MODE_MOUSEX10)) {
		if (state & GDK_SHIFT_MASK) code += 4;
		if (state & GDK_MOD1_MASK) code += 8;
		if (state & GDK_CONTROL_MASK) code += 16;
	}
	char bytes[64];
	int length;
	if (mode & MODE_MOUSESGR)
		length = g_snprintf(bytes, sizeof(bytes), "\033[<%d;%d;%d%c", code, x + 1, y + 1, release ? 'm' : 'M');
	else {
		if (x >= 223 || y >= 223) return;
		memcpy(bytes, "\033[M", 3);
		bytes[3] = 32 + (release ? 3 : code); bytes[4] = 33 + x; bytes[5] = 33 + y;
		length = 6;
	}
	input(t, bytes, length);
}

static gboolean
button(GtkWidget *widget, GdkEventButton *e, Terminal *t)
{
	gtk_widget_grab_focus(widget);
	int x, y;
	mouse_position(t, e->x, e->y, &x, &y);
	gboolean release = e->type == GDK_BUTTON_RELEASE;
	if ((t->page->mode & MODE_MOUSE) && !(e->state & GDK_SHIFT_MASK)) {
		t->button = release ? 0 : e->button;
		mouse_report(t, e->button - 1, x, y, e->state, release, FALSE);
		return TRUE;
	}
	tsessionuse(t->session);
	if (e->button == 1) {
		if (release) {
			selextend(x, y, e->state & GDK_MOD1_MASK ? SEL_RECTANGULAR : SEL_REGULAR, 1);
			t->selecting = FALSE;
			terminal_copy(t, GDK_SELECTION_PRIMARY);
		} else {
			selstart(x, y, e->type == GDK_2BUTTON_PRESS ? SNAP_WORD :
			                 e->type == GDK_3BUTTON_PRESS ? SNAP_LINE : 0);
			t->selecting = TRUE;
		}
		gtk_widget_queue_draw(widget);
	} else if (e->button == 2 && release) {
		terminal_paste(t, GDK_SELECTION_PRIMARY);
	}
	return TRUE;
}

static gboolean
motion(GtkWidget *widget, GdkEventMotion *e, Terminal *t)
{
	int x, y;
	mouse_position(t, e->x, e->y, &x, &y);
	if ((t->page->mode & MODE_MOUSE) && !(e->state & GDK_SHIFT_MASK)) {
		if (x != t->last_x || y != t->last_y)
			mouse_report(t, t->button ? t->button - 1 : 3, x, y, e->state, FALSE, TRUE);
		t->last_x = x; t->last_y = y;
	} else if (t->selecting) {
		tsessionuse(t->session);
		selextend(x, y, e->state & GDK_MOD1_MASK ? SEL_RECTANGULAR : SEL_REGULAR, 0);
		gtk_widget_queue_draw(widget);
	}
	return TRUE;
}

static gboolean
wheel(GtkWidget *widget, GdkEventScroll *e, Terminal *t)
{
	(void)widget;
	gboolean up = e->direction == GDK_SCROLL_UP || (e->direction == GDK_SCROLL_SMOOTH && e->delta_y < 0);
	if (e->direction == GDK_SCROLL_SMOOTH && e->delta_y == 0) return TRUE;
	if ((t->page->mode & MODE_MOUSE) && !(e->state & GDK_SHIFT_MASK)) {
		int x, y;
		mouse_position(t, e->x, e->y, &x, &y);
		mouse_report(t, up ? 64 : 65, x, y, e->state, FALSE, FALSE);
	} else if (t->frame.alternate && (t->page->mode & MODE_ALTSCROLL)) {
		input(t, t->page->mode & MODE_APPCURSOR ? (up ? "\033OA" : "\033OB") :
		      (up ? "\033[A" : "\033[B"), 3);
	} else {
		scroll_view(t, up, theme_scroll_multiplier);
	}
	return TRUE;
}

static void
prepare(Terminal *t)
{
	if (!t->session) t->session = tsessionnew(t->frame.cols, t->frame.rows);
	if (t->layout) return;
	/* Catalogs include every tab. Allocate cells only on first use or frame;
	 * Pango layouts, input methods and blink timers wait until first use. */
	t->layout = gtk_widget_create_pango_layout(t->page->widget, "");
	t->im = gtk_im_multicontext_new();
	g_signal_connect(t->im, "commit", G_CALLBACK(commit), t);
	g_signal_connect(t->im, "preedit-changed", G_CALLBACK(preedit), t);
	t->timer = g_timeout_add(800, blink, t);
	terminal_zoom(t);
	app_startuptime("font");
}

static void
mapped(GtkWidget *widget, Terminal *t)
{
	(void)widget;
	prepare(t);
}

Terminal *
terminal_new(Page *page)
{
	Terminal *t = g_new0(Terminal, 1);
	t->page = page;
	t->cols = page->app->cols;
	t->rows = page->app->rows;
	t->numlock = TRUE;
	t->frame = (SessionFrame){.cols = t->cols, .rows = t->rows};
	page->widget = gtk_drawing_area_new();
	g_object_set_data(G_OBJECT(page->widget), "terminal", t);
	gtk_widget_set_can_focus(page->widget, TRUE);
	gtk_widget_add_events(page->widget, GDK_BUTTON_PRESS_MASK | GDK_BUTTON_RELEASE_MASK |
	    GDK_POINTER_MOTION_MASK | GDK_SCROLL_MASK | GDK_SMOOTH_SCROLL_MASK | GDK_FOCUS_CHANGE_MASK);
	g_signal_connect(page->widget, "map", G_CALLBACK(mapped), t);
	g_signal_connect(page->widget, "draw", G_CALLBACK(draw_terminal), t);
	g_signal_connect(page->widget, "size-allocate", G_CALLBACK(resize), t);
	g_signal_connect(page->widget, "focus-in-event", G_CALLBACK(focus), t);
	g_signal_connect(page->widget, "focus-out-event", G_CALLBACK(focus), t);
	g_signal_connect(page->widget, "key-press-event", G_CALLBACK(key_event), t);
	g_signal_connect(page->widget, "key-release-event", G_CALLBACK(key_event), t);
	g_signal_connect(page->widget, "button-press-event", G_CALLBACK(button), t);
	g_signal_connect(page->widget, "button-release-event", G_CALLBACK(button), t);
	g_signal_connect(page->widget, "motion-notify-event", G_CALLBACK(motion), t);
	g_signal_connect(page->widget, "scroll-event", G_CALLBACK(wheel), t);
	return t;
}

void
terminal_packet(Terminal *t, WirePacket *p)
{
	if (p->type == WIRE_FRAME) {
		uint32_t v[6];
		for (size_t i = 0; i < G_N_ELEMENTS(v); i++)
			if (!wire_get_u32(p, &v[i])) return;
		if (v[0] < 1 || v[0] > 400 || v[1] < 1 || v[1] > 200 || v[2] >= v[0] || v[3] >= v[1]) return;
		if (!t->session) {
			t->session = tsessionnew(v[0], v[1]);
			t->full_damage = TRUE;
		}
		if (v[0] != (uint32_t)t->frame.cols || v[1] != (uint32_t)t->frame.rows ||
		    v[4] != (uint32_t)t->frame.scroll || v[5] != (uint32_t)t->frame.alternate || !t->page->live)
			t->full_damage = TRUE;
		if (v[2] != (uint32_t)t->frame.cursor_x || v[3] != (uint32_t)t->frame.cursor_y) {
			t->damaged_rows[t->frame.cursor_y] = TRUE;
			t->damaged_rows[v[3]] = TRUE;
		}
		t->frame = (SessionFrame){v[0], v[1], v[2], v[3], v[4], v[5]};
		tsessionimportframe(t->session, &t->frame);
		t->page->live = TRUE;
	} else if (p->type == WIRE_ROW) {
		uint32_t row;
		if (!t->session || !wire_get_u32(p, &row) || row >= (uint32_t)t->frame.rows ||
		    p->len - p->pos != (size_t)t->frame.cols * 16) return;
		Glyph cells[400];
		for (int x = 0; x < t->frame.cols; x++) {
			uint32_t mode;
			wire_get_u32(p, &cells[x].u); wire_get_u32(p, &mode);
			cells[x].mode = mode;
			wire_get_u32(p, &cells[x].fg); wire_get_u32(p, &cells[x].bg);
		}
		tsessionimportrow(t->session, row, cells, t->frame.cols);
		t->damaged_rows[row] = TRUE;
	} else if (p->type == WIRE_FRAME_FINISH) {
		app_startuptime("frame");
		/* Publish damage only when the service's complete frame has arrived. */
		gboolean changed = t->full_damage;
		if (t->full_damage) gtk_widget_queue_draw(t->page->widget);
		else for (int row = 0; row < t->frame.rows; row++)
			if (t->damaged_rows[row]) { damage_row(t, row); changed = TRUE; }
		t->full_damage = FALSE;
		memset(t->damaged_rows, 0, sizeof(t->damaged_rows));
		gboolean selection_ready = t->selection_pending && t->frame.cols == t->cols && t->frame.rows == t->rows;
		if (changed && (t->input_pending || selection_ready) && gtk_widget_get_mapped(t->page->widget)) {
			/* Complete interactive responses need not wait another refresh.
			 * Bulk output and rapid selections retain GTK's frame clock. */
			t->input_pending = FALSE;
			if (selection_ready) t->selection_pending = FALSE;
			G_GNUC_BEGIN_IGNORE_DEPRECATIONS
			gdk_window_process_updates(gtk_widget_get_window(t->page->widget), TRUE);
			G_GNUC_END_IGNORE_DEPRECATIONS
		}
	}
}

void
terminal_free(Terminal *t)
{
	g_object_set_data(G_OBJECT(t->page->widget), "terminal", NULL);
	if (t->timer) g_source_remove(t->timer);
	g_signal_handlers_disconnect_by_data(t->page->widget, t);
	g_clear_object(&t->im);
	clear_layouts(t);
	g_clear_object(&t->layout);
	if (t->font) pango_font_description_free(t->font);
	g_free(t->preedit);
	if (t->session) tsessionremove(t->session);
	g_free(t);
}

/* Imported terminal proxies never perform local PTY I/O or parser drawing. */
void xbell(void) {}
void xclipcopy(void) {}
void xsetsel(char *value) { free(value); }
void xsettitle(char *value) { (void)value; }
void xseticontitle(char *value) { (void)value; }
void xsetmode(int set, unsigned int flags) { (void)set; (void)flags; }
void xsetpointermotion(int set) { (void)set; }
int xsetcursor(int cursor) { (void)cursor; return 0; }
int xsetcolorname(int index, const char *name) { (void)index; (void)name; return 0; }
int xgetcolor(int i, unsigned char *r, unsigned char *g, unsigned char *b)
{ (void)i; (void)r; (void)g; (void)b; return 1; }
void xloadcols(void) {}
int xstartdraw(void) { return 0; }
int xoverlap(void) { return 0; }
void xdrawline(Line line, int x1, int y1, int x2, int pass)
{ (void)line; (void)x1; (void)y1; (void)x2; (void)pass; }
void xdrawcursor(int x, int y, Glyph glyph, int oldx, int oldy, Glyph old)
{ (void)x; (void)y; (void)glyph; (void)oldx; (void)oldy; (void)old; }
void xfinishdraw(void) {}
void xximspot(int x, int y) { (void)x; (void)y; }
