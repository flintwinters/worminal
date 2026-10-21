/* Draw the actual release module without a daemon, shell, browser or test renderer. */
#include <gmodule.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>
#include "../src/gtk/preview.h"
#include "win.h"

static void (*packet)(Terminal *, WirePacket *);

static void
deliver(Terminal *terminal, uint32_t type, WireBuffer *buffer)
{
	WirePacket message = {.type = type, .data = buffer->data, .len = buffer->len};
	packet(terminal, &message);
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
           gboolean unicode, gboolean clipped, const char *prefix, int frames)
{
	WireBuffer frame = {0};
	uint32_t values[] = {cols, rows, 0, 0, 0, 0};
	for (size_t i = 0; i < G_N_ELEMENTS(values); i++) wire_put_u32(&frame, values[i]);
	deliver(terminal, WIRE_FRAME, &frame);
	for (int row = 0; row < rows; row++) {
		WireBuffer cells = {0};
		wire_put_u32(&cells, row);
		for (int col = 0; col < cols; col++) {
			uint32_t rune = '!' + col % 94;
			if (unicode && col % 17 == 0) rune = 0x03bb;
			uint32_t mode = (row & 1 ? ATTR_BOLD : 0) | (row & 2 ? ATTR_ITALIC : 0);
			if (col % 7 == 0) mode |= ATTR_UNDERLINE;
			if (col % 13 == 0) mode |= ATTR_STRUCK;
			if (col % 11 == 0) mode |= ATTR_REVERSE;
			wire_put_u32(&cells, rune); wire_put_u32(&cells, mode);
			wire_put_u32(&cells, 1 + col % 15); wire_put_u32(&cells, 0);
		}
		deliver(terminal, WIRE_ROW, &cells);
	}
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
	const char *name = clipped ? "clipped_ascii" : unicode ? "mixed_unicode" : "dense_ascii";
	char *path = g_strdup_printf("%s-%s.png", prefix, name);
	g_assert_cmpint(cairo_surface_write_to_png(image, path), ==, CAIRO_STATUS_SUCCESS);
	g_print("{\"case\":\"%s\",\"frames\":%d,\"cols\":%d,\"rows\":%d,\"wall_us\":%" G_GINT64_FORMAT
	    ",\"cpu_ns\":%" G_GINT64_FORMAT "}", name, frames, cols, rows, elapsed, cpu);
	g_free(path);
	cairo_destroy(cr); cairo_surface_destroy(image);
}

int
main(int argc, char **argv)
{
	gtk_init(&argc, &argv);
	if (!g_strcmp0(g_getenv("GDK_BACKEND"), "wayland")) {
		g_assert_null(g_getenv("DISPLAY"));
		g_assert_cmpstr(G_OBJECT_TYPE_NAME(gdk_display_get_default()), ==, "GdkWaylandDisplay");
	}
	if (argc != 4) return 2;
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
	paint_case(page.terminal, page.widget, cols, rows, FALSE, FALSE, argv[2], frames);
	g_print(",");
	paint_case(page.terminal, page.widget, cols, rows, TRUE, FALSE, argv[2], frames);
	g_print(",");
	paint_case(page.terminal, page.widget, cols, rows, FALSE, TRUE, argv[2], frames);
	g_print("]\n");
	fflush(stdout);
	/* Let the parent sample memory while the shaped layouts are still alive. */
	(void)getchar();
	destroy(page.terminal);
	g_clear_pointer(&app.cached_font, pango_font_description_free);
	gtk_widget_destroy(app.window);
	g_module_close(module);
	return 0;
}
