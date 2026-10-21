/* Draw the actual release module without a daemon, shell, browser or test renderer. */
#include <gmodule.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include "../src/gtk/preview.h"
#include "win.h"
#include "st_state.h"

static void (*packet)(Terminal *, WirePacket *);
static TermSession *(*current_session)(void);

static void
import_packet(Terminal *terminal, uint32_t type, WireBuffer *buffer)
{
	WirePacket message = {.type = type, .data = buffer->data, .len = buffer->len};
	packet(terminal, &message);
}

static void
deliver(Terminal *terminal, uint32_t type, WireBuffer *buffer)
{
	import_packet(terminal, type, buffer);
	wire_buffer_free(buffer);
	*buffer = (WireBuffer){0};
}

static gint64
process_ns(void)
{
	struct timespec now;
	g_assert_cmpint(clock_gettime(CLOCK_PROCESS_CPUTIME_ID, &now), ==, 0);
	return (gint64)now.tv_sec * 1000000000 + now.tv_nsec;
}

static void
paint_case(Terminal *terminal, GtkWidget *widget, int cols, int rows,
           int content, gboolean clipped, const char *prefix, int frames)
{
	const gunichar runes[] = {0x03bb, 0x00e9, 0x0416, 0x05d0, 0x0639, 0x253c, 0x4e2d, 0x1f642};
	uint32_t coverage = 0;
	WireBuffer frame = {0};
	uint32_t values[] = {cols, rows, 0, 0, 0, 0};
	for (size_t i = 0; i < G_N_ELEMENTS(values); i++) wire_put_u32(&frame, values[i]);
	deliver(terminal, WIRE_FRAME, &frame);
	for (int row = 0; row < rows; row++) {
		WireBuffer cells = {0};
		wire_put_u32(&cells, row);
		gboolean dummy = FALSE;
		int glyph = row;
		uint32_t dummy_mode = 0, dummy_fg = 0;
		for (int col = 0; col < cols; col++) {
			gboolean is_dummy = dummy;
			uint32_t fg = 1 + col % 15;
			uint32_t rune = '!' + col % 94;
			if (content == 1 && col % 17 == 0) rune = 0x03bb;
			uint32_t mode = (row & 1 ? ATTR_BOLD : 0) | (row & 2 ? ATTR_ITALIC : 0);
			if (content == 2) {
				if (dummy) { rune = 0; mode |= ATTR_WDUMMY; dummy = FALSE; }
				else {
					unsigned int index = glyph++ % G_N_ELEMENTS(runes);
					rune = runes[index];
					if (g_unichar_iswide(rune)) {
						if (col + 1 < cols) { mode |= ATTR_WIDE; dummy = TRUE; }
						else rune = 0x03bb;
					}
					if (rune == runes[index]) coverage |= 1u << index;
				}
			}
			if (col % 7 == 0) mode |= ATTR_UNDERLINE;
			if (col % 13 == 0) mode |= ATTR_STRUCK;
			if (col % 11 == 0) mode |= ATTR_REVERSE;
			if (is_dummy) { mode = dummy_mode | ATTR_WDUMMY; fg = dummy_fg; }
			else if (mode & ATTR_WIDE) { dummy_mode = mode & ~ATTR_WIDE; dummy_fg = fg; }
			wire_put_u32(&cells, rune); wire_put_u32(&cells, mode);
			wire_put_u32(&cells, fg); wire_put_u32(&cells, 0);
		}
		deliver(terminal, WIRE_ROW, &cells);
	}
	if (content == 2) g_assert_cmpuint(coverage, ==, (1u << G_N_ELEMENTS(runes)) - 1);
	deliver(terminal, WIRE_FRAME_FINISH, &frame);
	cairo_surface_t *image = cairo_image_surface_create(CAIRO_FORMAT_ARGB32,
	    gtk_widget_get_allocated_width(widget), gtk_widget_get_allocated_height(widget));
	cairo_t *cr = cairo_create(image);
	if (clipped) {
		cairo_rectangle(cr, 0, gtk_widget_get_allocated_height(widget) / 2,
		    gtk_widget_get_allocated_width(widget), 16);
		cairo_clip(cr);
	}
	for (int i = 0; i < 20; i++) gtk_widget_draw(widget, cr);
	gint64 started_cpu = process_ns(), started = g_get_monotonic_time();
	for (int i = 0; i < frames; i++) gtk_widget_draw(widget, cr);
	gint64 elapsed = g_get_monotonic_time() - started, cpu = process_ns() - started_cpu;
	const char *name = clipped ? "clipped_ascii" :
	    content == 2 ? "dense_unicode" : content == 1 ? "mixed_unicode" : "dense_ascii";
	char *path = g_strdup_printf("%s-%s.png", prefix, name);
	g_assert_cmpint(cairo_surface_write_to_png(image, path), ==, CAIRO_STATUS_SUCCESS);
	g_print("{\"case\":\"%s\",\"frames\":%d,\"cols\":%d,\"rows\":%d,\"wall_us\":%" G_GINT64_FORMAT
	    ",\"cpu_ns\":%" G_GINT64_FORMAT "}", name, frames, cols, rows, elapsed, cpu);
	g_free(path);
	cairo_destroy(cr); cairo_surface_destroy(image);
}

static void
await_sample(void)
{
	g_print("]\n");
	fflush(stdout);
	(void)getchar();
}

static void
tabs_case(GModule *module, int count, const char *prefix, int frames,
          gboolean (*palette)(Page *, uint32_t, const char *))
{
	GtkWidget *(*create)(App *);
	void (*refresh)(App *);
	g_assert_true(g_module_symbol(module, "tabs_new", (gpointer *)&create));
	g_assert_true(g_module_symbol(module, "tabs_refresh", (gpointer *)&refresh));
	App app = {.fd = -1, .cols = 80, .rows = 1};
	Page *pages = g_new0(Page, count);
	for (int i = 0; i < count; i++) {
		pages[i].app = &app; pages[i].id = i + 1;
		char *name = g_strdup_printf("project-%05d", i);
		pages[i].label = gtk_label_new(name);
		g_object_ref_sink(pages[i].label);
		g_free(name);
		app.pages = g_list_append(app.pages, &pages[i]);
	}
	app.active = &pages[count - 1];
	for (size_t i = 0; i < G_N_ELEMENTS(app.active->colors); i++) palette(app.active, i, NULL);
	app.window = gtk_window_new(GTK_WINDOW_TOPLEVEL);
	app.tabstrip = create(&app);
	gtk_container_add(GTK_CONTAINER(app.window), app.tabstrip);
	refresh(&app);
	gtk_window_set_default_size(GTK_WINDOW(app.window), 80 * app.font_cw + 4, app.font_ch + 2);
	gtk_widget_show_all(app.window);
	gint64 settle = g_get_monotonic_time() + 200000;
	while (g_get_monotonic_time() < settle) {
		while (g_main_context_iteration(NULL, FALSE)) {}
		g_usleep(1000);
	}
	int width = gtk_widget_get_allocated_width(app.tabstrip), height = gtk_widget_get_allocated_height(app.tabstrip);
	cairo_surface_t *image = cairo_image_surface_create(CAIRO_FORMAT_ARGB32, width, height);
	cairo_t *cr = cairo_create(image);
	for (int i = 0; i < 20; i++) gtk_widget_draw(app.tabstrip, cr);
	gint64 start_cpu = process_ns(), start_wall = g_get_monotonic_time();
	for (int i = 0; i < frames; i++) gtk_widget_draw(app.tabstrip, cr);
	gint64 wall = g_get_monotonic_time() - start_wall, cpu = process_ns() - start_cpu;
	char *path = g_strdup_printf("%s-tab_strip.png", prefix);
	g_assert_cmpint(cairo_surface_write_to_png(image, path), ==, CAIRO_STATUS_SUCCESS);
	g_print("[{\"case\":\"tab_strip\",\"tabs\":%d,\"frames\":%d,\"cols\":%d,\"rows\":%d,"
	    "\"geometry_units\":\"pixels\",\"wall_us\":%" G_GINT64_FORMAT ",\"cpu_ns\":%" G_GINT64_FORMAT "}",
	    count, frames, width, height, wall, cpu);
	await_sample();
	g_free(path); cairo_destroy(cr); cairo_surface_destroy(image);
	gtk_widget_destroy(app.window);
	for (int i = 0; i < count; i++) g_object_unref(pages[i].label);
	g_list_free(app.pages); g_free(pages);
	g_clear_pointer(&app.cached_font, pango_font_description_free);
}

/* A cycle imports two complete owner frames; it never parses or paints them. */
static void
resize_case(Terminal *terminal, GtkWidget *widget, int cols, int rows, const char *prefix, int frames)
{
	int widths[] = {cols, MAX(1, cols * 2 / 3)};
	WireBuffer headers[2] = {{0}}, finish = {0};
	WireBuffer *lines = g_new0(WireBuffer, 2 * rows);
	for (int variant = 0; variant < 2; variant++) {
		uint32_t metadata[] = {widths[variant], rows, widths[variant] - 1, rows - 1, 0, 0};
		for (size_t i = 0; i < G_N_ELEMENTS(metadata); i++) wire_put_u32(&headers[variant], metadata[i]);
		for (int y = 0; y < rows; y++) {
			WireBuffer *line = &lines[variant * rows + y];
			wire_put_u32(line, y);
			for (int x = 0; x < widths[variant]; x++) {
				wire_put_u32(line, 'A' + x % 26);
				wire_put_u32(line, x + 1 == widths[variant] ? ATTR_WRAP : 0);
				wire_put_u32(line, 7); wire_put_u32(line, 0);
			}
		}
	}
	gint64 start_cpu = 0, start_wall = 0;
	for (int cycle = 0; cycle < 20 + frames; cycle++) {
		if (cycle == 20) { start_cpu = process_ns(); start_wall = g_get_monotonic_time(); }
		for (int variant = 0; variant < 2; variant++) {
			import_packet(terminal, WIRE_FRAME, &headers[variant]);
			for (int y = 0; y < rows; y++) import_packet(terminal, WIRE_ROW, &lines[variant * rows + y]);
			import_packet(terminal, WIRE_FRAME_FINISH, &finish);
		}
	}
	gint64 wall = g_get_monotonic_time() - start_wall, cpu = process_ns() - start_cpu;
	for (int variant = 0; variant < 2; variant++) wire_buffer_free(&headers[variant]);
	for (int y = 0; y < 2 * rows; y++) wire_buffer_free(&lines[y]);
	g_free(lines);
	TermSession *view = current_session();
	g_assert_cmpint(view->term.col, ==, widths[1]);
	g_assert_cmpint(view->term.row, ==, rows);
	int alternate_rows = view->term.alt ? rows : 0;
	size_t cells = (size_t)(rows + alternate_rows + view->term.histlen) * widths[1] * sizeof(Glyph);
	cairo_surface_t *image = cairo_image_surface_create(CAIRO_FORMAT_ARGB32,
	    gtk_widget_get_allocated_width(widget), gtk_widget_get_allocated_height(widget));
	cairo_t *cr = cairo_create(image);
	gtk_widget_draw(widget, cr);
	char *path = g_strdup_printf("%s-resize_import.png", prefix);
	g_assert_cmpint(cairo_surface_write_to_png(image, path), ==, CAIRO_STATUS_SUCCESS);
	g_print("{\"case\":\"resize_import\",\"frames\":%d,\"cycles\":%d,\"cols\":%d,\"rows\":%d,\"wall_us\":%" G_GINT64_FORMAT
	    ",\"cpu_ns\":%" G_GINT64_FORMAT ",\"local_history_rows\":%d,\"alternate_rows\":%d,\"glyph_storage_bytes\":%zu}",
	    2 * frames, frames, widths[1], rows, wall, cpu, view->term.histlen, alternate_rows, cells);
	g_free(path); cairo_destroy(cr); cairo_surface_destroy(image);
}

int
main(int argc, char **argv)
{
	gtk_init(&argc, &argv);
	if (!g_strcmp0(g_getenv("GDK_BACKEND"), "wayland")) {
		g_assert_null(g_getenv("DISPLAY"));
		g_assert_cmpstr(G_OBJECT_TYPE_NAME(gdk_display_get_default()), ==, "GdkWaylandDisplay");
	}
	if (argc != 4 && (argc != 5 || strcmp(argv[4], "--resize")) &&
	    (argc != 6 || strcmp(argv[4], "--tabs"))) return 2;
	int frames = atoi(argv[3]);
	if (frames < 1) return 2;
	GModule *module = g_module_open(argv[1], G_MODULE_BIND_LAZY);
	if (!module) g_error("Cannot load release frontend: %s", g_module_error());
	Terminal *(*create)(Page *);
	void (*destroy)(Terminal *), (*dimensions)(Terminal *, int *, int *);
	gboolean (*palette)(Page *, uint32_t, const char *);
	g_assert_true(g_module_symbol(module, "terminal_new", (gpointer *)&create));
	g_assert_true(g_module_symbol(module, "terminal_free", (gpointer *)&destroy));
	g_assert_true(g_module_symbol(module, "terminal_dimensions", (gpointer *)&dimensions));
	g_assert_true(g_module_symbol(module, "terminal_palette", (gpointer *)&palette));
	g_assert_true(g_module_symbol(module, "terminal_packet", (gpointer *)&packet));
	g_assert_true(g_module_symbol(module, "tsessioncurrent", (gpointer *)&current_session));
	if (argc == 6) {
		int count = atoi(argv[5]);
		if (count < 1 || count > 4096) return 2;
		tabs_case(module, count, argv[2], frames, palette);
		g_module_close(module);
		return 0;
	}
	App app = {.fd = -1, .cols = 100, .rows = 32, .geometry_pending = TRUE};
	Page page = {.app = &app, .mode = MODE_HIDE, .cursor = 2};
	app.active = &page;
	for (size_t i = 0; i < G_N_ELEMENTS(page.colors); i++) palette(&page, i, NULL);
	page.terminal = create(&page);
	app.window = gtk_window_new(GTK_WINDOW_TOPLEVEL);
	gtk_window_set_default_size(GTK_WINDOW(app.window), 1004, 618);
	gtk_container_add(GTK_CONTAINER(app.window), page.widget);
	gtk_widget_show_all(app.window);
	gint64 settle = g_get_monotonic_time() + 200000;
	while (g_get_monotonic_time() < settle) {
		while (g_main_context_iteration(NULL, FALSE)) {}
		g_usleep(1000);
	}
	int cols, rows;
	dimensions(page.terminal, &cols, &rows);
	g_print("[");
	if (argc == 5) resize_case(page.terminal, page.widget, cols, rows, argv[2], frames);
	else {
		paint_case(page.terminal, page.widget, cols, rows, FALSE, FALSE, argv[2], frames);
		g_print(",");
		paint_case(page.terminal, page.widget, cols, rows, TRUE, FALSE, argv[2], frames);
		g_print(",");
		paint_case(page.terminal, page.widget, cols, rows, 2, FALSE, argv[2], frames);
		g_print(",");
		paint_case(page.terminal, page.widget, cols, rows, FALSE, TRUE, argv[2], frames);
	}
	/* Let the parent sample memory while the shaped layouts are still alive. */
	await_sample();
	destroy(page.terminal);
	g_clear_pointer(&app.cached_font, pango_font_description_free);
	gtk_widget_destroy(app.window);
	g_module_close(module);
	return 0;
}
