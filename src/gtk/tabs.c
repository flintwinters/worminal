/* Match the original one-cell strip: directory names, underlines, inverted active tab. */
#include "preview.h"

typedef struct {
	App *app;
	PangoLayout *layout;
	int cw, ch, height, ascent;
} TabStrip;

static int
tab_width(TabStrip *strip, Page *page)
{
	int width;
	pango_layout_set_text(strip->layout, gtk_label_get_text(GTK_LABEL(page->label)), -1);
	pango_layout_get_pixel_size(strip->layout, &width, NULL);
	return width + 2 * strip->cw;
}

static GList *
first_visible(TabStrip *strip, int right)
{
	GList *first = g_list_find(strip->app->pages, strip->app->active);
	if (!first) first = g_list_last(strip->app->pages);
	if (!first) return NULL;
	/* Only the suffix ending at the active tab determines the left edge.
	 * Hidden prefixes need no text shaping. An oversized active tab clips. */
	int width = 2 + tab_width(strip, first->data);
	while (first->prev && width <= right) {
		int previous = tab_width(strip, first->prev->data);
		if (previous > right - width) break;
		width += previous;
		first = first->prev;
	}
	return first;
}

static gboolean
tabs_draw(GtkWidget *widget, cairo_t *cr, TabStrip *strip)
{
	App *app = strip->app;
	Page *active = app->active;
	if (!active) return FALSE;
	int right = gtk_widget_get_allocated_width(widget) - 2;
	gdk_cairo_set_source_rgba(cr, &active->colors[defaultbg]);
	cairo_paint(cr);
	cairo_rectangle(cr, 2, 2, MAX(0, right - 2), strip->ch);
	cairo_clip(cr);
	int x = 2, baseline = 2 + (strip->ch - strip->height) / 2 + strip->ascent;
	for (GList *item = first_visible(strip, right); item && x < right; item = item->next) {
		Page *page = item->data;
		int width = tab_width(strip, page);
		if (page == active) {
			gdk_cairo_set_source_rgba(cr, &active->colors[defaultfg]);
			cairo_rectangle(cr, x, 2, MIN(width, right - x), strip->ch);
			cairo_fill(cr);
		}
		gdk_cairo_set_source_rgba(cr, &active->colors[page == active ? defaultbg : defaultfg]);
		cairo_move_to(cr, x + strip->cw, baseline - pango_layout_get_baseline(strip->layout) / (double)PANGO_SCALE);
		pango_cairo_show_layout(cr, strip->layout);
		cairo_rectangle(cr, x + strip->cw, MIN(baseline + 2, strip->ch), MAX(0, width - 2 * strip->cw), 1);
		cairo_fill(cr);
		x += width;
	}
	return FALSE;
}

static gboolean
tabs_click(GtkWidget *widget, GdkEventButton *event, TabStrip *strip)
{
	if (!strip->layout || event->button != 1 || event->y < 2 || event->y >= 2 + strip->ch) return FALSE;
	int x = 2, right = gtk_widget_get_allocated_width(widget) - 2;
	for (GList *item = first_visible(strip, right); item && x < right; item = item->next) {
		Page *page = item->data;
		int width = tab_width(strip, page);
		if (event->x >= x && event->x < MIN(x + width, right)) {
			app_select(page);
			return TRUE;
		}
		x += width;
	}
	return FALSE;
}

void
tabs_refresh(App *app)
{
	if (!app->tabstrip || app->closing) return;
	TabStrip *strip = g_object_get_data(G_OBJECT(app->tabstrip), "tabs");
	if (!strip->layout) strip->layout = gtk_widget_create_pango_layout(app->tabstrip, "");
	PangoFontDescription *font = terminal_font(app, strip->layout,
	    &strip->cw, &strip->ch, &strip->height, &strip->ascent);
	pango_font_description_free(font);
	gtk_widget_set_size_request(app->tabstrip, -1, strip->ch + 2);
	gtk_widget_queue_draw(app->tabstrip);
}

static void
tabs_free(gpointer data)
{
	TabStrip *strip = data;
	g_clear_object(&strip->layout);
	g_free(strip);
}

GtkWidget *
tabs_new(App *app)
{
	GtkWidget *widget = gtk_drawing_area_new();
	TabStrip *strip = g_new0(TabStrip, 1);
	strip->app = app;
	g_object_set_data_full(G_OBJECT(widget), "tabs", strip, tabs_free);
	gtk_widget_add_events(widget, GDK_BUTTON_PRESS_MASK);
	g_signal_connect(widget, "draw", G_CALLBACK(tabs_draw), strip);
	g_signal_connect(widget, "button-press-event", G_CALLBACK(tabs_click), strip);
	return widget;
}
