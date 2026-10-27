/* Exercise the real frontend widgets and service together, on either GDK backend. */
#include "../src/gtk/app.c"
#include "../src/gtk/terminal.c"
#include "../src/gtk/browser.c"
#include "../src/gtk/tabs.c"
#include "../src/gtk/windows.c"
#include "../src/gtk/window_state.c"
#include <glib/gstdio.h>
#include "preview_text.c"
#include "preview_tabs.c"

static void diagnostics(void);

#define WAIT(condition) do { \
	gint64 deadline = g_get_monotonic_time() + 10000000; \
	while (!(condition) && g_get_monotonic_time() < deadline) { \
		while (g_main_context_iteration(NULL, FALSE)) {} \
		g_usleep(1000); \
	} \
	if (!(condition)) { diagnostics(); g_error("Timed out: %s (line %d)", #condition, __LINE__); } \
} while (0)

static void
diagnostics(void)
{
	int i = 0;
	for (GList *window = apps; window; window = window->next, i++) {
		App *app = window->data;
		g_printerr("window %d fd=%d reader=%u writer=%u queued=%zu active=%u\n", i,
		    app->fd, app->reader, app->writer, app->output_queue.len, app->active ? app->active->id : 0);
		for (GList *item = app->pages; item; item = item->next) {
			Page *page = item->data;
			Terminal *t = page->terminal;
			g_printerr("tab %u url=%s browser=%d\n", page->id, page->url ?: "(terminal)", page->browser != NULL);
			if (t) g_printerr("tab %u live=%d allocated=%dx%d frame=%dx%d scroll=%d\n",
			    page->id, page->live, t->cols, t->rows, t->frame.cols, t->frame.rows, t->frame.scroll);
			if (t && t->session) {
				GString *text = g_string_new(NULL);
				for (int y = 0; y < MIN(t->frame.rows, 6); y++) {
					Line line = tsessionviewline(t->session, y);
					for (int x = 0; x < t->frame.cols; x++) g_string_append_unichar(text, line[x].u);
					g_string_append_c(text, '\n');
				}
				g_printerr("%s", text->str);
				g_string_free(text, TRUE);
			}
		}
	}
}

static gboolean
screen_has(Terminal *t, const char *wanted)
{
	if (!t->session) return FALSE;
	GString *screen = g_string_new(NULL);
	for (int y = 0; y < t->frame.rows; y++) {
		Line line = tsessionviewline(t->session, y);
		for (int x = 0; x < t->frame.cols; x++)
			if (!(line[x].mode & ATTR_WDUMMY)) g_string_append_unichar(screen, line[x].u);
	}
	gboolean found = strstr(screen->str, wanted) != NULL;
	g_string_free(screen, TRUE);
	return found;
}

typedef struct { Terminal *terminal; const char *wanted; gboolean painted, partial; } PaintProbe;

static gboolean
paint_probe(GtkWidget *widget, cairo_t *cr, PaintProbe *probe)
{
	if (probe->wanted && screen_has(probe->terminal, probe->wanted)) {
		double x1, y1, x2, y2;
		cairo_clip_extents(cr, &x1, &y1, &x2, &y2);
		probe->partial |= y2 - y1 < gtk_widget_get_allocated_height(widget);
		probe->painted = TRUE;
	}
	return FALSE;
}

static void
reference_backgrounds(Terminal *t, cairo_t *cr, int first, int end)
{
	for (int y = first; y < end; y++) {
		Line line = tsessionviewline(t->session, y);
		for (int x = 0; x < t->frame.cols; x++) {
			GdkRGBA fg, bg;
			cell_colors(t, line[x], x, y, &fg, &bg);
			gdk_cairo_set_source_rgba(cr, &bg);
			cairo_rectangle(cr, 2 + x * t->cw, 2 + y * t->ch, t->cw, t->ch);
			cairo_fill(cr);
		}
	}
}

static void
check_background_spans(Terminal *t)
{
	tsessionuse(t->session);
	int width = gtk_widget_get_allocated_width(t->page->widget);
	int height = gtk_widget_get_allocated_height(t->page->widget);
	Glyph saved[400];
	Line line = tsessionviewline(t->session, 0);
	memcpy(saved, line, t->frame.cols * sizeof(*line));
	GdkRGBA saved_color = t->page->colors[1];
	t->page->colors[1].alpha = .5;
	for (int x = 0; x < t->frame.cols; x++) {
		line[x].bg = x / 5 % 16;
		if (x % 7 == 0) line[x].mode ^= ATTR_REVERSE;
	}
	const double scales[] = {1, 2, 1.25, 1};
	for (size_t index = 0; index < G_N_ELEMENTS(scales); index++) {
		int factor = 2;
		cairo_surface_t *surfaces[2];
		for (int reference = 0; reference < 2; reference++) {
			surfaces[reference] = cairo_image_surface_create(CAIRO_FORMAT_ARGB32, width * factor, height * factor);
			cairo_t *cr = cairo_create(surfaces[reference]);
			cairo_scale(cr, scales[index], scales[index]);
			if (index >= 2) cairo_translate(cr, .25, .5);
			cairo_rectangle(cr, 3.5, 4.25, width - 7, height - 8);
			cairo_clip(cr);
			if (reference) reference_backgrounds(t, cr, 0, t->frame.rows);
			else draw_backgrounds(t, cr, 0, t->frame.rows);
			cairo_destroy(cr);
			cairo_surface_flush(surfaces[reference]);
		}
		int bytes = cairo_image_surface_get_stride(surfaces[0]) * height * factor;
		g_assert_cmpmem(cairo_image_surface_get_data(surfaces[0]), bytes,
		    cairo_image_surface_get_data(surfaces[1]), bytes);
		for (int i = 0; i < 2; i++) cairo_surface_destroy(surfaces[i]);
	}
	memcpy(line, saved, t->frame.cols * sizeof(*line));
	t->page->colors[1] = saved_color;
	cairo_surface_t *surface = cairo_image_surface_create(CAIRO_FORMAT_ARGB32, width, height);
	cairo_t *cr = cairo_create(surface);
	gint64 elapsed[2] = {0};
	for (int i = 0; i < 60; i++) {
		for (int order = 0; order < 2; order++) {
			int reference = (i + order) % 2;
			gint64 start = g_get_monotonic_time();
			if (reference) reference_backgrounds(t, cr, 0, t->frame.rows);
			else draw_backgrounds(t, cr, 0, t->frame.rows);
			elapsed[reference] += g_get_monotonic_time() - start;
		}
	}
	g_print("Background paint: scalar %.2f ms, spans %.2f ms mean (60 alternating trials)\n",
	    elapsed[1] / 60000.0, elapsed[0] / 60000.0);
	cairo_destroy(cr);
	cairo_surface_destroy(surface);
}

static void
check_clipped_paint(Terminal *t)
{
	check_background_spans(t);
	int width = gtk_widget_get_allocated_width(t->page->widget);
	int height = gtk_widget_get_allocated_height(t->page->widget);
	cairo_surface_t *full = cairo_image_surface_create(CAIRO_FORMAT_ARGB32, width, height);
	cairo_surface_t *partial = cairo_image_surface_create(CAIRO_FORMAT_ARGB32, width, height);
	cairo_t *cr = cairo_create(full);
	draw_terminal(t->page->widget, cr, t);
	cairo_destroy(cr);
	cr = cairo_create(partial);
	/* Non-cell-aligned clips exercise glyphs that spill across compact rows. */
	for (int y = 0; y < height; y += 7) {
		cairo_save(cr);
		cairo_rectangle(cr, 0, y, width, 7);
		cairo_clip(cr);
		draw_terminal(t->page->widget, cr, t);
		cairo_restore(cr);
	}
	cairo_surface_flush(full);
	cairo_surface_flush(partial);
	g_assert_cmpmem(cairo_image_surface_get_data(full), cairo_image_surface_get_stride(full) * height,
	    cairo_image_surface_get_data(partial), cairo_image_surface_get_stride(partial) * height);
	for (int clipped = 0; clipped < 2; clipped++) {
		cairo_save(cr);
		if (clipped) {
			cairo_rectangle(cr, 0, height / 2, width, t->ch + 2 * glyph_overlap(t));
			cairo_clip(cr);
		}
		gint64 start = g_get_monotonic_time();
		for (int i = 0; i < 30; i++) draw_terminal(t->page->widget, cr, t);
		g_print("Dense GTK %s paint: %.2f ms mean\n", clipped ? "row" : "full",
		    (g_get_monotonic_time() - start) / 30000.0);
		cairo_restore(cr);
	}
	cairo_destroy(cr);
	cairo_surface_destroy(full);
	cairo_surface_destroy(partial);
}

static void
input_paint_latency(Terminal *t)
{
	PaintProbe probe = {.terminal = t};
	gulong handler = g_signal_connect_after(t->page->widget, "draw", G_CALLBACK(paint_probe), &probe);
	GString *wanted = g_string_new("KEYPAINT");
	GString *dense = g_string_new("\033[2J\033[H");
	for (int y = 0; y < t->frame.rows - 2; y++) {
		g_string_append(dense, "\033[1;3;4m");
		for (int x = 0; x < t->frame.cols - 2; x++) g_string_append_c(dense, 'A' + x % 26);
		g_string_append(dense, "\033[0m\r\n");
	}
	input(t, dense->str, dense->len);
	g_string_free(dense, TRUE);
	input(t, "\r\nKEYPAINT", 10);
	WAIT(screen_has(t, wanted->str));
	check_clipped_paint(t);
	check_ascii_line_rendering(t);
	check_unicode_layout_rendering(t);
	/* Input acknowledgements may carry unchanged frame metadata before the
	 * PTY echoes anything. They must not consume the prompt-paint allowance. */
	WireBuffer unchanged = {0};
	int metadata[] = {t->frame.cols, t->frame.rows, t->frame.cursor_x, t->frame.cursor_y,
	                  t->frame.scroll, t->frame.alternate};
	for (size_t i = 0; i < G_N_ELEMENTS(metadata); i++) wire_put_u32(&unchanged, metadata[i]);
	WirePacket packet = {.type = WIRE_FRAME, .data = unchanged.data, .len = unchanged.len};
	t->input_pending = TRUE;
	t->selection_pending = TRUE;
	terminal_packet(t, &packet);
	packet.type = WIRE_FRAME_FINISH;
	terminal_packet(t, &packet);
	g_assert_true(t->input_pending);
	g_assert_true(t->selection_pending);
	/* Selection responses must wait for frame completion and current geometry,
	 * independently of the existing keyboard-response allowance. */
	WireBuffer row = {0};
	wire_put_u32(&row, 1);
	Line cells = tsessionviewline(t->session, 1);
	for (int x = 0; x < t->frame.cols; x++) {
		wire_put_u32(&row, cells[x].u); wire_put_u32(&row, cells[x].mode);
		wire_put_u32(&row, cells[x].fg); wire_put_u32(&row, cells[x].bg);
	}
	int cols = t->cols;
	t->input_pending = FALSE;
	probe.wanted = wanted->str;
	probe.painted = FALSE;
	for (int pass = 0; pass < 2; pass++) {
		t->cols = cols + !pass;
		packet = (WirePacket){.type = WIRE_FRAME, .data = unchanged.data, .len = unchanged.len};
		terminal_packet(t, &packet);
		packet = (WirePacket){.type = WIRE_ROW, .data = row.data, .len = row.len};
		terminal_packet(t, &packet);
		g_assert_false(probe.painted);
		packet.type = WIRE_FRAME_FINISH;
		terminal_packet(t, &packet);
		g_assert_cmpint(probe.painted, ==, pass != 0);
		g_assert_cmpint(t->selection_pending, ==, pass == 0);
	}
	wire_buffer_free(&row);
	wire_buffer_free(&unchanged);
	gint64 elapsed = 0;
	for (int i = 0; i < 20; i++) {
		g_string_append_c(wanted, 'a' + i);
		probe.wanted = wanted->str;
		probe.painted = FALSE;
		gint64 start = g_get_monotonic_time();
		input(t, wanted->str + wanted->len - 1, 1);
		WAIT(probe.painted);
		g_assert_false(t->input_pending);
		g_assert_false(t->selection_pending);
		elapsed += g_get_monotonic_time() - start;
	}
	g_print("Input to GTK paint: %.2f ms mean (20 echoes)\n", elapsed / 20000.0);
	g_assert_true(probe.partial);
	g_signal_handler_disconnect(t->page->widget, handler);
	g_string_free(wanted, TRUE);
}

static void select_page(Page *);

typedef struct { guint draws; gboolean partial; } DamageProbe;

static gboolean
damage_probe(GtkWidget *widget, cairo_t *cr, DamageProbe *probe)
{
	double x1, y1, x2, y2;
	cairo_clip_extents(cr, &x1, &y1, &x2, &y2);
	probe->draws++;
	probe->partial = y2 - y1 < gtk_widget_get_allocated_height(widget);
	return FALSE;
}

static void
flush_paint(Terminal *t)
{
	G_GNUC_BEGIN_IGNORE_DEPRECATIONS
	gdk_window_process_updates(gtk_widget_get_window(t->page->widget), TRUE);
	G_GNUC_END_IGNORE_DEPRECATIONS
}

#include "preview_ownership.c"

static void
check_blink_damage(void)
{
	App app = {.fd = -1, .cols = 80, .rows = 24};
	Page page = {.app = &app, .live = TRUE, .cursor = 2};
	app.active = &page;
	for (size_t i = 0; i < G_N_ELEMENTS(page.colors); i++) terminal_palette(&page, i, NULL);
	Terminal *t = terminal_new(&page);
	page.terminal = t;
	app.window = gtk_window_new(GTK_WINDOW_TOPLEVEL);
	gtk_window_set_default_size(GTK_WINDOW(app.window), 804, 474);
	gtk_container_add(GTK_CONTAINER(app.window), page.widget);
	DamageProbe probe = {0};
	g_signal_connect_after(page.widget, "draw", G_CALLBACK(damage_probe), &probe);
	gtk_widget_show_all(app.window);
	WAIT(probe.draws);
	g_source_remove(t->timer); t->timer = 0;
	/* Wayland configures, focus and buffer releases can arrive after the
	 * first draw. Establish an idle baseline before counting blink damage. */
	gint64 quiet = g_get_monotonic_time(), deadline = quiet + G_TIME_SPAN_SECOND;
	guint previous = probe.draws;
	do {
		while (g_main_context_iteration(NULL, FALSE)) {}
		flush_paint(t);
		if (probe.draws != previous) { previous = probe.draws; quiet = g_get_monotonic_time(); }
		g_usleep(1000);
	} while (g_get_monotonic_time() - quiet < 100000 && g_get_monotonic_time() < deadline);
	g_assert_cmpint(g_get_monotonic_time() - quiet, >=, 100000);
	probe.draws = 0;
	gint64 start = g_get_monotonic_time();
	for (int i = 0; i < 20; i++) { blink(t); flush_paint(t); }
	g_assert_cmpuint(probe.draws, ==, 0);
	g_print("Steady idle blink: %u redraws in 20 ticks (%.2f ms total)\n",
	    probe.draws, (g_get_monotonic_time() - start) / 1000.0);
	/* Focus-following pointer entry changes the cursor, not the whole viewport. */
	GdkEventFocus focus_event = {.type = GDK_FOCUS_CHANGE, .in = TRUE};
	focus(page.widget, &focus_event, t);
	flush_paint(t);
	g_assert_cmpuint(probe.draws, ==, 1);
	g_assert_true(probe.partial);
	focus_event.in = FALSE;
	probe.draws = 0;
	focus(page.widget, &focus_event, t);
	flush_paint(t);
	g_assert_cmpuint(probe.draws, ==, 1);
	g_assert_true(probe.partial);
	probe.draws = 0;
	/* Metadata alone must paint immediately without waiting for blink or
	 * inventing a row update. Unchanged metadata must not damage anything. */
	terminal_configure(t, page.mode, page.cursor, FALSE);
	flush_paint(t);
	g_assert_cmpuint(probe.draws, ==, 0);
	terminal_configure(t, MODE_HIDE, page.cursor, FALSE);
	flush_paint(t);
	g_assert_cmpuint(probe.draws, ==, 1);
	g_assert_true(probe.partial);
	probe.draws = 0;
	for (int i = 0; i < 20; i++) { blink(t); flush_paint(t); }
	g_assert_cmpuint(probe.draws, ==, 0);
	terminal_configure(t, 0, 1, FALSE);
	flush_paint(t);
	probe.draws = 0;
	blink(t); flush_paint(t);
	g_assert_cmpuint(probe.draws, ==, 1);
	g_assert_true(probe.partial);
	terminal_configure(t, MODE_HIDE, 2, FALSE);
	flush_paint(t);
	Line line = tsessionviewline(t->session, 2);
	line[2].u = 'A'; line[2].mode = ATTR_BLINK;
	probe.draws = 0;
	blink(t); flush_paint(t);
	g_assert_cmpuint(probe.draws, ==, 1);
	g_assert_true(probe.partial);
	gboolean changed = terminal_palette(&page, defaultbg, "#123456");
	g_assert_true(changed);
	g_assert_false(terminal_palette(&page, defaultbg, "#123456"));
	probe.draws = 0;
	terminal_configure(t, page.mode, page.cursor, changed);
	flush_paint(t);
	g_assert_cmpuint(probe.draws, ==, 1);
	g_assert_false(probe.partial);
	probe.draws = 0;
	terminal_configure(t, page.mode | MODE_REVERSE, page.cursor, FALSE);
	flush_paint(t);
	g_assert_cmpuint(probe.draws, ==, 1);
	g_assert_false(probe.partial);
	/* Selection only reserves an allowance for an eventual service response.
	 * It must not flush cached or partially imported cells during mapping. */
	int cols = t->cols, rows = t->rows;
	t->cols = t->frame.cols; t->rows = t->frame.rows;
	for (int guard = 0; guard < 3; guard++) {
		probe.draws = 0;
		t->cols = t->frame.cols + (guard == 1);
		page.live = guard != 2;
		app.last_selection = 0;
		gtk_widget_queue_draw(page.widget);
		terminal_select(t);
		g_assert_cmpuint(probe.draws, ==, 0);
		g_assert_cmpint(t->selection_pending, ==, guard != 0);
		flush_paint(t);
		g_assert_cmpuint(probe.draws, ==, 1);
		t->selection_pending = FALSE;
	}
	/* Keep a queued rapid-repeat selection on GTK's normal frame clock.
	 * A future timestamp makes this check independent of test preemption. */
	probe.draws = 0;
	app.last_selection = g_get_monotonic_time() + 10 * G_TIME_SPAN_SECOND;
	gtk_widget_queue_draw(page.widget);
	terminal_select(t);
	g_assert_false(t->selection_pending);
	g_assert_cmpuint(probe.draws, ==, 0);
	flush_paint(t);
	t->cols = cols; t->rows = rows;
	page.live = TRUE;
	check_passive_ownership(t);
	gtk_widget_hide(app.window);
	probe.draws = 0;
	for (int i = 0; i < 20; i++) { blink(t); flush_paint(t); }
	g_assert_cmpuint(probe.draws, ==, 0);
	terminal_free(t);
	g_clear_pointer(&app.cached_font, pango_font_description_free);
	gtk_widget_destroy(app.window);
}

static void
check_initial_dimensions(void)
{
	App app = {.cols = 93, .rows = 17};
	Page page = {.app = &app};
	Terminal *t = terminal_new(&page);
	g_object_ref_sink(page.widget);
	g_assert_null(t->layout);
	g_assert_null(t->session);
	g_assert_null(t->im);
	g_assert_cmpuint(t->timer, ==, 0);
	/* Font setup before realization must not send GTK's placeholder 1x1
	 * allocation to a running shell as its initial terminal geometry. */
	terminal_zoom(t);
	g_assert_nonnull(t->session);
	g_assert_cmpint(tsessionframe(t->session).cols, ==, app.cols);
	g_assert_cmpint(tsessionframe(t->session).rows, ==, app.rows);
	int cols, rows;
	terminal_dimensions(t, &cols, &rows);
	g_assert_cmpint(cols, ==, app.cols);
	g_assert_cmpint(rows, ==, app.rows);
	terminal_free(t);
	g_clear_pointer(&app.cached_font, pango_font_description_free);
	gtk_widget_destroy(page.widget);
	g_object_unref(page.widget);
	/* A catalog tab may disappear before it is ever selected. */
	t = terminal_new(&page);
	g_object_ref_sink(page.widget);
	terminal_free(t);
	gtk_widget_destroy(page.widget);
	g_object_unref(page.widget);
	/* A complete frame may arrive for an unmapped view. Import its actual
	 * geometry without preparing GTK fonts or accepting a row before it. */
	t = terminal_new(&page);
	g_object_ref_sink(page.widget);
	WirePacket packet = {.type = WIRE_ROW};
	terminal_packet(t, &packet);
	g_assert_null(t->session);
	WireBuffer frame = {0};
	uint32_t values[] = {7, 3, 6, 2, 0, 0};
	for (size_t i = 0; i < G_N_ELEMENTS(values); i++) wire_put_u32(&frame, values[i]);
	packet = (WirePacket){.type = WIRE_FRAME, .data = frame.data, .len = frame.len};
	terminal_packet(t, &packet);
	g_assert_nonnull(t->session);
	g_assert_null(t->layout);
	g_assert_cmpint(tsessionframe(t->session).cols, ==, 7);
	g_assert_cmpint(tsessionframe(t->session).rows, ==, 3);
	packet.type = WIRE_FRAME_FINISH;
	terminal_packet(t, &packet);
	wire_buffer_free(&frame);
	terminal_free(t);
	gtk_widget_destroy(page.widget);
	g_object_unref(page.widget);
}

static void
check_connection_gate(void)
{
	App *app = new_app();
	/* Fail before any transport/daemon work. A nested main loop must not
	 * dispatch the failure callback against an unfinished window. */
	app->master = g_strdup("-invalid");
	ConnectionRequest *request = connect_async(app, TRUE);
	gint64 deadline = g_get_monotonic_time() + 100000;
	while (g_get_monotonic_time() < deadline) {
		while (g_main_context_iteration(NULL, FALSE)) {}
		g_usleep(1000);
	}
	g_assert_cmpint(app->refs, ==, 2);
	g_assert_cmpint(app->fd, ==, -1);
	g_assert_cmpuint(app->reconnect, ==, 0);
	app->closing = TRUE;
	g_mutex_unlock(&request->widgets_ready);
	WAIT(app->refs == 1);
	apps = g_list_remove(apps, app);
	app_unref(app);
	app = new_app();
	app->master = g_strdup("-invalid");
	show_app(app);
	WAIT(app->reconnect);
	g_assert_cmpint(app->fd, ==, -1);
	gtk_widget_destroy(app->window);
}

static void
check_tabs(App *app, Page *terminal, Page *browser)
{
	TabStrip *strip = g_object_get_data(G_OBJECT(app->tabstrip), "tabs");
	g_assert_cmpint(strip->cw, ==, terminal->terminal->cw);
	g_assert_cmpint(strip->ch, ==, terminal->terminal->ch);
	g_assert_cmpint(gtk_widget_get_allocated_height(app->tabstrip), ==, strip->ch + 2);
	g_assert_cmpstr(gtk_label_get_text(GTK_LABEL(browser->label)), ==, "Fixture");
	int width = gtk_widget_get_allocated_width(app->tabstrip), height = strip->ch + 2;
	cairo_surface_t *image = cairo_image_surface_create(CAIRO_FORMAT_ARGB32, width, height);
	cairo_t *cr = cairo_create(image);
	tabs_draw(app->tabstrip, cr, strip);
	g_assert_cmpint(cairo_surface_write_to_png(image, ".checks/preview-tabs.png"), ==, CAIRO_STATUS_SUCCESS);
	cairo_destroy(cr); cairo_surface_destroy(image);
	/* Hit testing uses exactly the same variable widths and visible range as drawing. */
	GList *first = first_visible(strip, width - 2);
	GdkEventButton click = {.button = 1, .x = 3, .y = 3};
	g_assert_true(tabs_click(app->tabstrip, &click, strip));
	g_assert_true(app->active == first->data);
	select_page(browser);
	/* Long names scroll the strip to keep the active tab visible. */
	char *name = g_strdup(gtk_label_get_text(GTK_LABEL(terminal->label)));
	gtk_label_set_text(GTK_LABEL(terminal->label), "a-very-long-directory-name-before-the-browser-tab");
	g_assert_true(first_visible(strip, 20)->data == browser);
	select_page(terminal);
	gtk_label_set_text(GTK_LABEL(terminal->label), "a-very-long-directory-name-before-the-browser-tab");
	g_assert_true(first_visible(strip, 20)->data == terminal);
	select_page(browser);
	gtk_label_set_text(GTK_LABEL(terminal->label), name);
	g_free(name);
}

#include "preview_browser.c"

static void
select_page(Page *page)
{
	gtk_window_present(GTK_WINDOW(page->app->window));
	if (page->url) app_select(page);
	else gtk_notebook_set_current_page(GTK_NOTEBOOK(page->app->notebook),
	    gtk_notebook_page_num(GTK_NOTEBOOK(page->app->notebook), page->widget));
	WAIT(page->app->active == page && gtk_widget_get_mapped(page->widget));
	if (page->terminal) g_assert_true(gtk_window_get_focus(GTK_WINDOW(page->app->window)) == page->widget);
}

static void
press_browser_key(Page *page, guint key, guint mods)
{
	GdkEventKey event = {.type = GDK_KEY_PRESS, .keyval = key, .state = mods};
	g_assert_true(shortcut(page->app->window, &event, page->app));
}

#include "preview_find.c"

static Page *
open_browser(App *app, const char *url)
{
	int before = g_list_length(app->pages);
	app_browser(app, url);
	WAIT(g_list_length(app->pages) == before + 1 && app->active && app->active->browser);
	Page *page = app->active;
	WAIT(!webkit_web_view_is_loading(page->browser->view) &&
	     !g_strcmp0(webkit_web_view_get_uri(page->browser->view), page->url) &&
	     script_equals(page->browser, "document.readyState", "complete"));
	g_assert_null(page->browser->entry);
	gtk_widget_show_all(browser_widget(page->browser));
	g_assert_null(page->browser->entry);
	return page;
}

static GtkWidget *
dialog_of_type(GType type)
{
	GList *windows = gtk_window_list_toplevels();
	GtkWidget *found = NULL;
	for (GList *item = windows; item; item = item->next)
		if (g_type_is_a(G_OBJECT_TYPE(item->data), type) && gtk_widget_get_visible(item->data))
			found = item->data;
	g_list_free(windows);
	return found;
}

static gboolean
has_file(const char *path)
{
	gchar *contents = NULL;
	gsize size;
	gboolean okay = g_file_get_contents(path, &contents, &size, NULL) &&
	                !g_strcmp0(contents, "Worminal download fixture\n");
	g_free(contents);
	return okay;
}

static void
finished(WebKitWebView *view, WebKitLoadEvent event, gboolean *done)
{
	(void)view;
	if (event == WEBKIT_LOAD_FINISHED) *done = TRUE;
}

static void
reload_wait(Browser *b)
{
	gboolean done = FALSE;
	gulong handler = g_signal_connect(b->view, "load-changed", G_CALLBACK(finished), &done);
	webkit_web_view_reload(b->view);
	WAIT(done);
	g_signal_handler_disconnect(b->view, handler);
}

#include "preview_media.c"

static void
paint(Terminal *t, const char *name)
{
	GtkAllocation allocation;
	gtk_widget_get_allocation(t->page->widget, &allocation);
	cairo_surface_t *surface = cairo_image_surface_create(CAIRO_FORMAT_ARGB32, allocation.width, allocation.height);
	cairo_t *cr = cairo_create(surface);
	draw_terminal(t->page->widget, cr, t);
	gint64 start = g_get_monotonic_time();
	for (int i = 0; i < 30; i++) draw_terminal(t->page->widget, cr, t);
	g_print("Terminal repaint: %.2f ms/frame\n", (g_get_monotonic_time() - start) / 30000.0);
	/* Cached and freshly shaped layouts must paint exactly the same pixels. */
	cairo_surface_t *reference = cairo_image_surface_create(CAIRO_FORMAT_ARGB32, allocation.width, allocation.height);
	cairo_t *fresh = cairo_create(reference);
	clear_layouts(t);
	draw_terminal(t->page->widget, fresh, t);
	cairo_surface_flush(surface);
	cairo_surface_flush(reference);
	g_assert_cmpmem(cairo_image_surface_get_data(surface),
	    cairo_image_surface_get_stride(surface) * allocation.height,
	    cairo_image_surface_get_data(reference),
	    cairo_image_surface_get_stride(reference) * allocation.height);
	cairo_destroy(fresh); cairo_surface_destroy(reference);
	g_assert_cmpint(cairo_status(cr), ==, CAIRO_STATUS_SUCCESS);
	g_assert_cmpint(cairo_surface_write_to_png(surface, name), ==, CAIRO_STATUS_SUCCESS);
	cairo_destroy(cr); cairo_surface_destroy(surface);
}

static void
check_outline(App *app)
{
	gtk_container_check_resize(GTK_CONTAINER(app->window));
	GtkWidget *content = gtk_bin_get_child(GTK_BIN(app->window));
	int width = gtk_widget_get_allocated_width(content), height = gtk_widget_get_allocated_height(content);
	cairo_surface_t *surface = cairo_image_surface_create(CAIRO_FORMAT_ARGB32, width, height);
	cairo_t *cr = cairo_create(surface);
	gtk_widget_draw(content, cr);
	cairo_surface_flush(surface);
	unsigned char *pixels = cairo_image_surface_get_data(surface);
	int stride = cairo_image_surface_get_stride(surface);
	uint32_t expected = gtk_window_is_active(GTK_WINDOW(app->window)) ? 0xff999999 : 0xff4c4c4c;
	for (int y = 0; y < height; y++) {
		uint32_t *row = (uint32_t *)(pixels + y * stride);
		for (int x = 0; x < width; x++)
			if (!x || !y || x == width - 1 || y == height - 1)
				g_assert_cmphex(row[x], ==, expected);
	}
	cairo_destroy(cr);
	cairo_surface_destroy(surface);
}

static void
check_outline_focus(App *app)
{
	GtkWidget *other = gtk_window_new(GTK_WINDOW_TOPLEVEL);
	gtk_widget_show(other);
	gdk_window_focus(gtk_widget_get_window(other), GDK_CURRENT_TIME);
	WAIT(gtk_window_is_active(GTK_WINDOW(other)) &&
	    !gtk_window_is_active(GTK_WINDOW(app->window)));
	check_outline(app);
	gdk_window_focus(gtk_widget_get_window(app->window), GDK_CURRENT_TIME);
	WAIT(gtk_window_is_active(GTK_WINDOW(app->window)));
	check_outline(app);
	gtk_widget_destroy(other);
}

int
main(int argc, char **argv)
{
	setlocale(LC_CTYPE, "");
	gtk_init(&argc, &argv);
	static const BrowserHost callbacks = {
		app_page_title, app_page_url, app_startuptime, app_browser, app_close, app_select
	};
	browser_initialize(&callbacks);
	check_tab_ranges();
	check_catalog_tab_refresh();
	check_catalog_selection_claim();
	check_initial_dimensions();
	check_blink_damage();
	check_connection_gate();
	const char *backend = g_getenv("GDK_BACKEND");
	if (backend && !strcmp(backend, "wayland")) {
		g_assert_null(g_getenv("DISPLAY"));
		g_assert_cmpstr(G_OBJECT_TYPE_NAME(gdk_display_get_default()), ==, "GdkWaylandDisplay");
	}
	App *app = new_app();
	app->title = g_strdup("Preview integration");
	const char *master = g_getenv("WORMINAL_PREVIEW_TEST_MASTER");
	app->master = g_strdup(master);
	char *command[] = {"/bin/sh", "-c", "stty raw -echo; printf READY; exec cat", NULL};
	app->command = g_strdupv(command);
	show_app(app);
	g_assert_null(((TabStrip *)g_object_get_data(G_OBJECT(app->tabstrip), "tabs"))->layout);
	GdkEvent *early = gdk_event_new(GDK_KEY_PRESS);
	early->key.window = g_object_ref(gtk_widget_get_window(app->window));
	early->key.keyval = GDK_KEY_q;
	early->key.hardware_keycode = 24;
	shortcut(app->window, &early->key, app);
	gdk_event_free(early);
	WAIT(app->active && app->active->terminal);
	Page *terminal = app->active;
	Terminal *t = terminal->terminal;
	WAIT(screen_has(t, "READY"));
	WAIT(screen_has(t, "q"));
	check_outline(app);
	input_paint_latency(t);
	/* Terminal titles belong to the window; directory basenames name tabs. */
	app_page_title(terminal, "Shell");
	int label_length;
	const char *label_name = tab_label(terminal->directory, &label_length);
	char *expected_label = g_strndup(label_name, label_length);
	g_assert_cmpstr(gtk_label_get_text(GTK_LABEL(terminal->label)), ==, expected_label);
	g_assert_cmpstr(gtk_window_get_title(GTK_WINDOW(app->window)), ==, "Shell");
	g_assert_false(gtk_notebook_get_show_tabs(GTK_NOTEBOOK(app->notebook)));
	g_free(expected_label);
	input(t, " hello UTF-8: \303\251 \344\270\255", strlen(" hello UTF-8: \303\251 \344\270\255"));
	WAIT(screen_has(t, "hello UTF-8: \303\251 \344\270\255"));
	g_assert_cmpuint(t->page->mode & MODE_MOUSE, ==, 0);
	GdkEventKey key = {.type = GDK_KEY_PRESS, .keyval = GDK_KEY_End, .state = GDK_CONTROL_MASK};
	g_assert_cmpstr(keymap(t, &key), ==, "\033[1;5F");
	g_signal_emit_by_name(t->im, "commit", " IME");
	WAIT(screen_has(t, "IME"));
	input(t, "\r\nSELECT-ME", 11);
	WAIT(screen_has(t, "SELECT-ME"));
	tsessionuse(t->session);
	selstart(0, t->frame.cursor_y, 0);
	selextend(8, t->frame.cursor_y, SEL_REGULAR, 0);
	selextend(8, t->frame.cursor_y, SEL_REGULAR, 1);
	terminal_copy(t, GDK_SELECTION_CLIPBOARD);
	terminal_paste(t, GDK_SELECTION_CLIPBOARD);
	WAIT(screen_has(t, "SELECT-MESELECT-ME"));
	int oldcols = t->frame.cols;
	gtk_window_resize(GTK_WINDOW(app->window), 720, 420);
	if (!g_getenv("WORMINAL_PREVIEW_TILED")) WAIT(t->frame.cols != oldcols && t->frame.cols == t->cols);
	check_outline(app);
	oldcols = t->frame.cols;
	app->zoom = 2;
	terminal_zoom(t);
	for (size_t style = 0; style < G_N_ELEMENTS(t->ascii); style++)
		for (size_t c = 0; c < G_N_ELEMENTS(t->ascii[style]); c++)
			g_assert_null(t->ascii[style][c]);
	WAIT(t->frame.cols != oldcols && t->frame.cols == t->cols);
	check_ascii_line_rendering(t);
	check_unicode_layout_rendering(t);
	paint(t, ".checks/preview-terminal.png");
	GString *flood = g_string_new(NULL);
	for (int i = 0; i < 2000; i++) g_string_append_printf(flood, "\r\noutput-%04d", i);
	g_string_append(flood, "\r\nDRAINED");
	input(t, flood->str, flood->len);
	g_string_free(flood, TRUE);
	WAIT(screen_has(t, "DRAINED"));
	paint(t, ".checks/preview-terminal-dense.png");
	WireBuffer scroll = {0};
	wire_put_u32(&scroll, 1); wire_put_u32(&scroll, 3);
	app_send(app, WIRE_SCROLL, terminal->id, scroll.data, scroll.len);
	WAIT(t->frame.scroll == 3);
	wire_buffer_free(&scroll);
	input(t, "\r\nLIVE", 6);
	WAIT(t->frame.scroll == 0 && screen_has(t, "LIVE"));
	/* A second window takes painting ownership; reconnect recovers the same tab. */
	App *peer = new_app();
	peer->master = g_strdup(master);
	peer->initial_attach = TRUE;
	show_app(peer);
	WAIT(peer->active && peer->active->id == terminal->id && !terminal->live &&
	     peer->active->terminal->frame.cols == peer->active->terminal->cols &&
	     ((!backend || strcmp(backend, "wayland")) && !g_getenv("WORMINAL_PREVIEW_TILED") ? TRUE :
	      gtk_window_is_active(GTK_WINDOW(peer->window))));
	select_page(terminal);
	input(t, "\r\nCONTROL", 9);
	WAIT(terminal->live && screen_has(t, "CONTROL"));
	disconnected(app);
	WAIT(app->fd >= 0 && app->attached && terminal->live);
	g_assert_cmpuint(app->active->id, ==, terminal->id);
	const char *base = g_getenv("WORMINAL_PREVIEW_FIXTURE");
	g_assert_nonnull(base);
	char *url = g_strconcat(base, "/page", NULL);
	check_file_views(app);
	check_browser_media(app);
	if (g_getenv("WORMINAL_PREVIEW_LOCAL_APP")) {
		check_explorer_refresh(app);
		Page *home = open_browser(app, g_getenv("WORMINAL_PREVIEW_LOCAL_APP"));
		check_explorer(home->browser);
		int home_count = g_list_length(app->pages);
		app_close(home);
		WAIT(g_list_length(app->pages) == home_count - 1);
	}
	Page *web = open_browser(app, url);
	g_assert_true(related_view(url) == web->browser->view);
	g_assert_null(related_view("https://unrelated.invalid/"));
	g_assert_null(related_view("http://127.0.0.1:1/"));
	g_assert_null(related_view("file:///etc/hosts"));
	g_assert_null(related_view("about:blank"));
	g_assert_null(related_view(WORKSPACE_BROWSER_HOME));
	expect_script(web->browser, "sessionStorage.setItem('private-tab', 'first'); 'ok'", "ok");
	Page *sibling = open_browser(app, url);
	expect_script(sibling->browser, "String(window.opener)", "null");
	expect_script(sibling->browser, "String(sessionStorage.getItem('private-tab'))", "null");
	int sibling_count = g_list_length(app->pages);
	app_close(sibling);
	WAIT(g_list_length(app->pages) == sibling_count - 1);
	select_page(web);
	check_tabs(app, terminal, web);
	check_browser_find(web);
	press_browser_key(web, GDK_KEY_k, GDK_CONTROL_MASK);
	g_assert_true(gtk_widget_get_visible(web->browser->entry));
	g_assert_true(gtk_window_get_focus(GTK_WINDOW(app->window)) == web->browser->entry);
	press_browser_key(web, GDK_KEY_Escape, 0);
	g_assert_false(gtk_widget_get_visible(web->browser->entry));
	press_browser_key(web, GDK_KEY_k, GDK_CONTROL_MASK);
	g_signal_emit_by_name(web->browser->entry, "activate");
	g_assert_false(gtk_widget_get_visible(web->browser->entry));
	WAIT(!webkit_web_view_is_loading(web->browser->view));
	check_outline(app);
	expect_script(web->browser, "document.title", "Fixture");

	expect_script(web->browser, "document.querySelector('input').value='local form'; localStorage.setItem('saved','yes');"
	              "document.cookie='session=local;path=/'; document.querySelector('input').value", "local form");
	WAIT(g_list_length(peer->pages) == 2);
	Page *shared = find_page(peer, web->id);
	g_assert_null(shared->browser); /* Shared links do not eagerly launch pages. */
	Browser *live_browser = web->browser;
	select_page(shared);
	WAIT(shared->browser == live_browser && !web->browser);
	expect_script(shared->browser, "document.title + ':' + document.querySelector('input').value", "Fixture:local form");
	char *other = g_strconcat(base, "/other", NULL);
	webkit_web_view_load_uri(shared->browser->view, other);
	WAIT(!webkit_web_view_is_loading(shared->browser->view) &&
	     !g_strcmp0(webkit_web_view_get_title(shared->browser->view), "Other"));
	WAIT(!g_strcmp0(web->url, other));
	press_browser_key(shared, GDK_KEY_Left, GDK_MOD1_MASK);
	WAIT(!webkit_web_view_is_loading(shared->browser->view) &&
	     !g_strcmp0(webkit_web_view_get_title(shared->browser->view), "Fixture"));
	select_page(web);
	WAIT(web->browser == live_browser && !shared->browser);
	expect_script(web->browser, "document.title + ':' + document.querySelector('input').value", "Fixture:local form");
	expect_script(web->browser, "localStorage.getItem('saved') + ':' + document.cookie", "yes:session=local");
	webkit_web_view_load_uri(web->browser->view, other);
	WAIT(!webkit_web_view_is_loading(web->browser->view) && webkit_web_view_can_go_back(web->browser->view));
	press_browser_key(web, GDK_KEY_Left, GDK_MOD1_MASK);
	WAIT(!webkit_web_view_is_loading(web->browser->view) &&
	     !g_strcmp0(webkit_web_view_get_title(web->browser->view), "Fixture"));
	press_browser_key(web, GDK_KEY_Right, GDK_MOD1_MASK);
	WAIT(!webkit_web_view_is_loading(web->browser->view) &&
	     !g_strcmp0(webkit_web_view_get_title(web->browser->view), "Other"));
	press_browser_key(web, GDK_KEY_Left, GDK_MOD1_MASK);
	WAIT(!webkit_web_view_is_loading(web->browser->view) &&
	     !g_strcmp0(webkit_web_view_get_title(web->browser->view), "Fixture"));
	reload_wait(web->browser);
	WAIT(gtk_widget_get_allocated_width(browser_view(web->browser)) > 300);
	/* User popup links become independent tabs; window.opener is not shared. */
	int before_popup = g_list_length(app->pages);
	expect_script(web->browser, "window.open('/other') === null ? 'blocked' : 'opened'", "blocked");
	WAIT(g_list_length(app->pages) == before_popup + 1 && app->active != web);
	Page *child = app->active;
	WAIT(child->browser && !webkit_web_view_is_loading(child->browser->view) &&
	     !g_strcmp0(webkit_web_view_get_title(child->browser->view), "Other"));
	app_close(child);
	WAIT(g_list_length(app->pages) == before_popup);
	select_page(web);
	webkit_web_view_terminate_web_process(web->browser->view);
	WAIT(gtk_widget_get_visible(web->browser->notice));
	press_browser_key(web, GDK_KEY_r, GDK_CONTROL_MASK);
	WAIT(!webkit_web_view_is_loading(web->browser->view) &&
	     !g_strcmp0(webkit_web_view_get_title(web->browser->view), "Fixture"));
	/* Permission decisions use a real WebKit request and a GTK response. */
	expect_script(web->browser, "Notification.requestPermission().then(x=>window.permissionResult=x); 'asked'", "asked");
	WAIT(dialog_of_type(GTK_TYPE_MESSAGE_DIALOG));
	gtk_dialog_response(GTK_DIALOG(dialog_of_type(GTK_TYPE_MESSAGE_DIALOG)), GTK_RESPONSE_CANCEL);
	/* The WebKit promise resolves asynchronously after the GTK dialog response. */
	WAIT(script_equals(web->browser, "window.permissionResult", "denied"));
	/* Complete an actual WebKit download through the save dialog. */
	char *download = g_build_filename(g_get_current_dir(), ".checks", "preview-download.txt", NULL);
	g_unlink(download);
	expect_script(web->browser, "document.querySelector('a[download]').click(); 'download'", "download");
	WAIT(dialog_of_type(GTK_TYPE_FILE_CHOOSER_DIALOG));
	GtkWidget *chooser = dialog_of_type(GTK_TYPE_FILE_CHOOSER_DIALOG);
	char *folder = g_path_get_dirname(download);
	gtk_file_chooser_set_current_folder(GTK_FILE_CHOOSER(chooser), folder);
	gtk_file_chooser_set_current_name(GTK_FILE_CHOOSER(chooser), "preview-download.txt");
	/* File chooser folder changes finish asynchronously; wait for its filename. */
	gint64 chooser_deadline = g_get_monotonic_time() + 5000000;
	gboolean chosen = FALSE;
	while (!chosen && g_get_monotonic_time() < chooser_deadline) {
		while (g_main_context_iteration(NULL, FALSE)) {}
		char *filename = gtk_file_chooser_get_filename(GTK_FILE_CHOOSER(chooser));
		chosen = !g_strcmp0(filename, download);
		g_free(filename);
		g_usleep(1000);
	}
	g_assert_true(chosen);
	gtk_dialog_response(GTK_DIALOG(chooser), GTK_RESPONSE_ACCEPT);
	WAIT(has_file(download));
	g_free(folder); g_free(download);
	/* A self-signed local server must display failure and never load the page. */
	const char *tls = g_getenv("WORMINAL_PREVIEW_TLS_FIXTURE");
	int before_tls = g_list_length(app->pages);
	app_browser(app, tls);
	WAIT(g_list_length(app->pages) == before_tls + 1 && app->active && app->active->browser);
	Page *failed = app->active;
	WAIT(script_equals(failed->browser, "String(document.body.textContent.includes('Certificate'))", "true"));
	expect_script(failed->browser, "getComputedStyle(document.documentElement).backgroundColor", "rgb(24, 24, 24)");
	g_assert_null(webkit_web_view_get_title(failed->browser->view));
	app_close(failed);
	WAIT(g_list_length(app->pages) == before_tls);
	uint32_t webid = web->id;
	app_close(web);
	WAIT(find_page(app, webid) == NULL && find_page(peer, webid) == NULL);
	web = open_browser(app, url);
	expect_script(web->browser, "localStorage.getItem('saved') + ':' + document.cookie", "yes:session=local");
	/* The shell command changes the existing record, then Ctrl+T always creates
	 * a fresh terminal. Ctrl+Shift+B has no browser action. */
	App *converter = new_app();
	converter->master = g_strdup(master);
	char *test_cwd = g_get_current_dir();
	char *frontend_path = g_build_filename(test_cwd, "worminal", NULL);
	g_free(test_cwd);
	char *quoted_frontend = g_shell_quote(frontend_path);
	char *convert_script = g_strdup_printf("stty raw -echo; printf CONVERT_READY; "
	    "dd bs=1 count=1 >/dev/null 2>&1; %s --new-browser-tab about:blank; sleep 30", quoted_frontend);
	char *convert_command[] = {"/bin/sh", "-c", convert_script, NULL};
	converter->command = g_strdupv(convert_command);
	show_app(converter);
	WAIT(converter->active && converter->active->terminal && screen_has(converter->active->terminal, "CONVERT_READY"));
	uint32_t converted_id = converter->active->id;
	WAIT(find_page(peer, converted_id));
	select_page(find_page(peer, converted_id));
	WAIT(peer->active->live);
	select_page(converter->active);
	app_focus(converter->active);
	WAIT(converter->active->live && !peer->active->live);
	input(converter->active->terminal, "x", 1);
	WAIT(converter->active && converter->active->id == converted_id && converter->active->browser);
	g_assert_null(converter->active->terminal);
	Browser *converted_browser = converter->active->browser;
	WAIT(peer->active && peer->active->id == converted_id && peer->active->url);
	g_assert_null(peer->active->browser);
	g_assert_null(converted_browser->entry);
	WAIT(gtk_window_get_focus(GTK_WINDOW(converter->window)) == browser_view(converted_browser));
	GdkEventKey control_t = {.type = GDK_KEY_PRESS, .keyval = GDK_KEY_t, .state = GDK_CONTROL_MASK};
	g_assert_true(shortcut(converter->window, &control_t, converter));
	WAIT(converter->active && converter->active->terminal && converter->active->id != converted_id);
	GdkEventKey removed = {.type = GDK_KEY_PRESS, .keyval = GDK_KEY_b,
	    .state = GDK_CONTROL_MASK | GDK_SHIFT_MASK};
	g_assert_false(shortcut(converter->window, &removed, converter));
	WAIT(find_page(app, converted_id));
	gtk_widget_destroy(converter->window);
	select_page(find_page(app, converted_id));
	WAIT(app->active->browser == converted_browser);
	g_free(convert_script); g_free(quoted_frontend); g_free(frontend_path);
	int before_window = g_list_length(app->pages);
	app_new_window(app);
	WAIT(windows == 3 && g_list_length(app->pages) == before_window + 1);
	GList *toplevels = gtk_window_list_toplevels();
	GtkWidget *extra = NULL;
	for (GList *item = toplevels; item; item = item->next)
		if (item->data != app->window && item->data != peer->window && gtk_widget_get_visible(item->data))
			extra = item->data;
	g_list_free(toplevels);
	g_assert_nonnull(extra);
	WAIT(gtk_window_is_active(GTK_WINDOW(extra)));
	gtk_widget_destroy(extra);
	g_assert_cmpint(windows, ==, 2);
	gtk_widget_destroy(peer->window);
	check_outline_focus(app);
	gtk_widget_destroy(app->window);
	g_free(url); g_free(other);
	g_print("Preview %s: terminal, resize, selection, clipboard, IME, ownership, reconnect, browser sharing, "
	        "conversion, find, media, "
	        "navigation, storage, popups, downloads, permissions, TLS rejection and renderer recovery passed.\n",
	        backend ? backend : "default");
	return 0;
}
