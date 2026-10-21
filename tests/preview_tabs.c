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
