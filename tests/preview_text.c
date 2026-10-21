/* Compare production ASCII drawing with the legacy full-layout oracle. */
static void
ascii_grid(Terminal *t, cairo_t *cr, gboolean lines, PangoLayout *legacy[4][95])
{
	for (int style = 0; style < 4; style++) {
		for (int col = 0; col < 95; col++) {
			double top = 2 + (style + 1) * t->ch - t->height - theme_glyph_offset_y;
			if (lines) {
				char value[] = {' ' + col, 0};
				text(t, cr, value, col, style, (style & 1 ? ATTR_BOLD : 0) | (style & 2 ? ATTR_ITALIC : 0));
			} else {
				cairo_move_to(cr, 2 + col * t->cw, top);
				pango_cairo_show_layout(cr, legacy[style][col]);
			}
		}
	}
}

static void
check_ascii_line_rendering(Terminal *t)
{
	cairo_surface_t *seed = cairo_image_surface_create(CAIRO_FORMAT_ARGB32, 1, 1);
	cairo_t *cr = cairo_create(seed);
	PangoLayout *legacy[4][95];
	for (int style = 0; style < 4; style++) {
		PangoFontDescription *font = pango_font_description_copy(t->font);
		pango_font_description_set_weight(font, style & 1 ? PANGO_WEIGHT_BOLD : PANGO_WEIGHT_NORMAL);
		pango_font_description_set_style(font, style & 2 ? PANGO_STYLE_ITALIC : PANGO_STYLE_NORMAL);
		for (int col = 0; col < 95; col++) {
			char value[] = {' ' + col, 0};
			legacy[style][col] = gtk_widget_create_pango_layout(t->page->widget, value);
			pango_layout_set_font_description(legacy[style][col], font);
			text(t, cr, value, 0, 0, (style & 1 ? ATTR_BOLD : 0) | (style & 2 ? ATTR_ITALIC : 0));
		}
		pango_font_description_free(font);
	}
	cairo_destroy(cr); cairo_surface_destroy(seed);
	int width = 2 * (95 * t->cw + 4), height = 2 * (4 * t->ch + t->height + 4);
	PangoContext *context = pango_layout_get_context(t->layout);
	PangoDirection direction = pango_context_get_base_dir(context);
	for (int rtl = 0; rtl < 2; rtl++) {
		pango_context_set_base_dir(context, rtl ? PANGO_DIRECTION_RTL : PANGO_DIRECTION_LTR);
		for (int transform = 0; transform < 3; transform++) {
			for (int clipped = 0; clipped < 2; clipped++) {
				cairo_surface_t *images[2];
				for (int lines = 0; lines < 2; lines++) {
					images[lines] = cairo_image_surface_create(CAIRO_FORMAT_ARGB32, width, height);
					cr = cairo_create(images[lines]);
					if (transform == 1) cairo_scale(cr, 2, 2);
					if (transform == 2) { cairo_translate(cr, .25, .5); cairo_scale(cr, 1.25, 1.25); }
					if (clipped) { cairo_rectangle(cr, 13, 7, width / 3, height / 3); cairo_clip(cr); }
					cairo_set_source_rgba(cr, .3, .7, .9, .5);
					ascii_grid(t, cr, lines, legacy);
					cairo_destroy(cr);
					cairo_surface_flush(images[lines]);
				}
				g_assert_cmpmem(cairo_image_surface_get_data(images[0]), cairo_image_surface_get_stride(images[0]) * height,
				    cairo_image_surface_get_data(images[1]), cairo_image_surface_get_stride(images[1]) * height);
				cairo_surface_destroy(images[0]); cairo_surface_destroy(images[1]);
			}
		}
	}
	pango_context_set_base_dir(context, direction);
	cairo_surface_t *image = cairo_image_surface_create(CAIRO_FORMAT_ARGB32, width, height);
	cr = cairo_create(image);
	cairo_set_source_rgb(cr, .3, .7, .9);
	ascii_grid(t, cr, FALSE, legacy); ascii_grid(t, cr, TRUE, legacy);
	gint64 elapsed[2] = {0};
	for (int trial = 0; trial < 60; trial++) {
		for (int variant = 0; variant < 2; variant++) {
			int lines = variant ^ (trial & 1);
			gint64 start = g_get_monotonic_time();
			ascii_grid(t, cr, lines, legacy);
			elapsed[lines] += g_get_monotonic_time() - start;
		}
	}
	g_print("ASCII paint: legacy layouts %.2f ms, production %.2f ms mean (60 alternating grids)\n",
	    elapsed[0] / 60000.0, elapsed[1] / 60000.0);
	cairo_destroy(cr); cairo_surface_destroy(image);
	for (int style = 0; style < 4; style++)
		for (int col = 0; col < 95; col++) g_object_unref(legacy[style][col]);
}
