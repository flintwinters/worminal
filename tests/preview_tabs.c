/* Prefix-width oracle independent of production's visible-range traversal. */
static void
check_tab_ranges(void)
{
	App app = {0};
	Page pages[64] = {{0}};
	int widths[64];
	GtkWidget *widget = gtk_drawing_area_new();
	g_object_ref_sink(widget);
	TabStrip strip = {.app = &app, .layout = gtk_widget_create_pango_layout(widget, "")};
	PangoFontDescription *font = terminal_font(&app, strip.layout,
	    &strip.cw, &strip.ch, &strip.height, &strip.ascent);
	pango_font_description_free(font);
	for (int i = 0; i < 64; i++) {
		char *name = i == 31 ? g_strnfill(300, 'W') : g_strdup_printf("project-%0*d-λ", 1 + i % 12, i);
		pages[i].app = &app; pages[i].id = i + 1;
		pages[i].label = gtk_label_new(name);
		g_object_ref_sink(pages[i].label);
		g_free(name);
		app.pages = g_list_append(app.pages, &pages[i]);
		widths[i] = tab_width(&strip, &pages[i]);
	}
	int rights[] = {320, 800, 4096, 40, 0};
	for (int index = 0; index < 65; index++) {
		int active = index == 64 ? 63 : (index + 31) % 64;
		app.active = index == 64 ? NULL : &pages[active];
		for (size_t test = 0; test < G_N_ELEMENTS(rights); test++) {
			int first = 0, width = 2;
			for (int i = 0; i <= active; i++) width += widths[i];
			/* An oversized active tab stays visible and is clipped in place. */
			while (first < active && width > rights[test]) width -= widths[first++];
			g_assert_true(first_visible(&strip, rights[test]) == g_list_nth(app.pages, first));
		}
	}
	g_list_free(app.pages); app.pages = NULL; app.active = NULL;
	g_assert_null(first_visible(&strip, 320));
	for (int i = 0; i < 64; i++) g_object_unref(pages[i].label);
	g_object_unref(strip.layout); g_object_unref(widget);
	g_clear_pointer(&app.cached_font, pango_font_description_free);
}

/* A catalog must refresh even when its selected page does not change. */
static void
check_catalog_tab_refresh(void)
{
	App app = {.fd = -1, .attached = TRUE};
	app.window = gtk_window_new(GTK_WINDOW_TOPLEVEL);
	app.notebook = gtk_notebook_new();
	gtk_container_add(GTK_CONTAINER(app.window), app.notebook);
	app.tabstrip = tabs_new(&app);
	g_object_ref_sink(app.tabstrip);
	Page page = {.app = &app, .id = 7, .url = g_strdup("about:blank"),
	    .widget = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0), .label = gtk_label_new("old")};
	gtk_notebook_append_page(GTK_NOTEBOOK(app.notebook), page.widget, page.label);
	app.pages = g_list_append(NULL, &page);
	app.active = &page;
	TabStrip *strip = g_object_get_data(G_OBJECT(app.tabstrip), "tabs");
	app.syncing = TRUE;
	page_title(&page, "pending catalog title");
	g_assert_null(strip->layout);
	app.syncing = FALSE;
	WireBuffer buffer = {0};
	wire_put_u32(&buffer, 1); wire_put_u32(&buffer, 0); wire_put_u32(&buffer, page.id);
	wire_put_u32(&buffer, page.id); wire_put_u32(&buffer, 1);
	wire_put_string(&buffer, "about:blank");
	wire_put_u32(&buffer, 0); wire_put_u32(&buffer, 2);
	wire_put_string(&buffer, "final catalog title"); wire_put_string(&buffer, "catalog-directory");
	wire_put_u32(&buffer, 0);
	WirePacket packet = {.type = WIRE_CATALOG, .data = buffer.data, .len = buffer.len};
	g_assert_true(catalog(&app, &packet));
	g_assert_false(app.syncing);
	g_assert_true(app.active == &page);
	g_assert_nonnull(strip->layout);
	g_assert_cmpint(strip->ch, >, 0);
	g_assert_cmpstr(gtk_label_get_text(GTK_LABEL(page.label)), ==, "final catalog title");
	g_assert_cmpstr(gtk_window_get_title(GTK_WINDOW(app.window)), ==, "final catalog title");
	wire_buffer_free(&buffer);
	gtk_widget_destroy(app.window); g_object_unref(app.tabstrip);
	g_list_free(app.pages);
	g_free(page.url); g_free(page.title); g_free(page.directory);
	g_clear_pointer(&app.cached_font, pango_font_description_free);
}

/* Notebook switching must not send a second claim during initial attach. */
static void
check_catalog_selection_claim(void)
{
	int sockets[2];
	g_assert_cmpint(socketpair(AF_UNIX, SOCK_STREAM, 0, sockets), ==, 0);
	App app = {.fd = sockets[0], .cols = 80, .rows = 24, .syncing = TRUE, .created_id = 2};
	app.window = gtk_window_new(GTK_WINDOW_TOPLEVEL);
	app.notebook = gtk_notebook_new();
	gtk_container_add(GTK_CONTAINER(app.window), app.notebook);
	app.tabstrip = tabs_new(&app);
	g_object_ref_sink(app.tabstrip);
	g_signal_connect_after(app.notebook, "switch-page", G_CALLBACK(switched), &app);
	Page *first = new_page(&app, 1, FALSE), *second = new_page(&app, 2, FALSE);
	app.active = first;
	app.syncing = FALSE;
	WireBuffer buffer = {0};
	wire_put_u32(&buffer, 2); wire_put_u32(&buffer, 0); wire_put_u32(&buffer, 1);
	for (uint32_t id = 1; id <= 2; id++) {
		wire_put_u32(&buffer, id); wire_put_u32(&buffer, 0);
		wire_put_string(&buffer, "");
		wire_put_u32(&buffer, 0); wire_put_u32(&buffer, 2);
		wire_put_string(&buffer, "catalog terminal"); wire_put_string(&buffer, "catalog-directory");
		wire_put_u32(&buffer, 0);
	}
	WirePacket catalog_packet = {.type = WIRE_CATALOG, .data = buffer.data, .len = buffer.len};
	g_assert_true(catalog(&app, &catalog_packet));
	g_assert_true(app.active == second);
	g_assert_true(app.attached);
	g_assert_false(app.syncing);
	WireReader reader = {0};
	WirePacket message;
	g_assert_cmpint(wire_tryread(sockets[1], &reader, &message), ==, 1);
	g_assert_cmpuint(message.type, ==, WIRE_FOCUS);
	g_assert_cmpuint(message.tab, ==, second->id);
	uint32_t cols, rows;
	g_assert_true(wire_get_u32(&message, &cols) && wire_get_u32(&message, &rows));
	g_assert_cmpuint(cols, ==, 80); g_assert_cmpuint(rows, ==, 24);
	wire_packet_free(&message);
	g_assert_cmpint(wire_tryread(sockets[1], &reader, &message), ==, 0);
	wire_reader_free(&reader); wire_buffer_free(&buffer);
	g_signal_handlers_disconnect_by_data(app.notebook, &app);
	app.active = NULL;
	page_free(first); page_free(second);
	gtk_widget_destroy(app.window); g_object_unref(app.tabstrip);
	g_list_free(app.pages);
	wire_buffer_free(&app.output_queue);
	g_clear_pointer(&app.cached_font, pango_font_description_free);
	close(sockets[0]); close(sockets[1]);
}

/* Verify bright selection borders and actual title ink in both selection states. */
static void
check_tab_title_colors(App *app, Page *terminal, Page *browser)
{
	TabStrip *strip = g_object_get_data(G_OBJECT(app->tabstrip), "tabs");
	GList *saved_pages = app->pages;
	Page *saved_active = app->active;
	Page *pages[] = {terminal, browser};
	int colors[] = {1, 7, defaultbg, defaultfg};
	GdkRGBA saved[2][G_N_ELEMENTS(colors)];
	char *names[2];
	GdkRGBA palette[] = {{1, 0, 0, 1}, {1, 1, 1, 1}, {0, 0, 0, 1}, {1, 1, 1, 1}};
	app->pages = NULL;
	for (int i = 0; i < 2; i++) {
		names[i] = g_strdup(gtk_label_get_text(GTK_LABEL(pages[i]->label)));
		gtk_label_set_text(GTK_LABEL(pages[i]->label), "MMMM");
		for (int c = 0; c < (int)G_N_ELEMENTS(colors); c++) {
			saved[i][c] = pages[i]->colors[colors[c]];
			pages[i]->colors[colors[c]] = palette[c];
		}
		app->pages = g_list_append(app->pages, pages[i]);
	}
	int width = gtk_widget_get_allocated_width(app->tabstrip), height = strip->ch + 2;
	int tabwidth = tab_width(strip, terminal);
	g_assert_cmpint(width, >, 2 * tabwidth + 4);
	for (int selected = 0; selected < 2; selected++) {
		app->active = pages[selected];
		cairo_surface_t *surface = cairo_image_surface_create(CAIRO_FORMAT_ARGB32, width, height);
		cairo_t *cr = cairo_create(surface);
		tabs_draw(app->tabstrip, cr, strip);
		cairo_surface_flush(surface);
		unsigned char *data = cairo_image_surface_get_data(surface);
		int stride = cairo_image_surface_get_stride(surface), red[2] = {0}, white[2] = {0};
		uint32_t *top = (uint32_t *)(data + 2 * stride);
		g_assert_cmphex(top[2 + selected * tabwidth + tabwidth / 2], ==, 0xffffffff);
		g_assert_cmphex(top[2 + (1 - selected) * tabwidth + tabwidth / 2], ==, 0xff000000);
		int baseline = 2 + (strip->ch - strip->height) / 2 + strip->ascent;
		for (int y = 4; y < MIN(baseline, height); y++) {
			uint32_t *row = (uint32_t *)(data + y * stride);
			for (int x = 2; x < 2 + 2 * tabwidth; x++) {
				int within_tab = (x - 2) % tabwidth;
				if (within_tab < strip->cw || within_tab >= tabwidth - strip->cw) continue;
				uint32_t pixel = row[x];
				int r = (pixel >> 16) & 255, g = (pixel >> 8) & 255, b = pixel & 255;
				if (r > g + 32 && r > b + 32) red[(x - 2) / tabwidth]++;
				if (r > 200 && abs(r - g) < 2 && abs(r - b) < 2) white[(x - 2) / tabwidth]++;
			}
		}
		g_assert_cmpint(red[0], ==, 0);
		g_assert_cmpint(red[1], >, 0);
		g_assert_cmpint(white[0], >, 0);
		g_assert_cmpint(white[1], ==, 0);
		cairo_destroy(cr); cairo_surface_destroy(surface);
	}
	g_list_free(app->pages);
	app->pages = saved_pages; app->active = saved_active;
	for (int i = 0; i < 2; i++) {
		gtk_label_set_text(GTK_LABEL(pages[i]->label), names[i]); g_free(names[i]);
		for (int c = 0; c < (int)G_N_ELEMENTS(colors); c++) pages[i]->colors[colors[c]] = saved[i][c];
	}
}
