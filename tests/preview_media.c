/* Exercise the native player controls and decoding, rather than canPlayType alone. */
static cairo_surface_t *media_image;
static void
media_snapshot(GObject *source, GAsyncResult *result, gpointer data)
{
	(void)data;
	GError *error = NULL;
	media_image = webkit_web_view_get_snapshot_finish(WEBKIT_WEB_VIEW(source), result, &error);
	g_assert_no_error(error);
}

static void
check_browser_media(App *app)
{
	char *cwd = g_get_current_dir();
	const char *names[] = {"sample.mp3", "sample.wav", "sample.mp4"};
	for (size_t kind = 0; kind < G_N_ELEMENTS(names); kind++) {
		gboolean video = kind == 2;
		char *path = g_build_filename(cwd, "tests/fixtures/media", names[kind], NULL);
		char *url = g_filename_to_uri(path, NULL, NULL);
		Page *page = open_browser(app, url);
		Browser *b = page->browser;
		g_assert_cmpstr(page->url, ==, url);
		expect_script(b, "String(!!document.querySelector('audio,video'))", "true");
		expect_script(b, "const media=document.querySelector('audio,video'); media.pause(); media.muted=false;"
		    "media.tabIndex=0; media.focus(); String(media.controls)", "true");
		gtk_window_present(GTK_WINDOW(app->window));
		WAIT(gtk_window_is_active(GTK_WINDOW(app->window)));
		gtk_widget_grab_focus(browser_view(b));
		media_image = NULL;
		webkit_web_view_get_snapshot(b->view, WEBKIT_SNAPSHOT_REGION_VISIBLE, WEBKIT_SNAPSHOT_OPTIONS_NONE,
		    NULL, media_snapshot, NULL);
		WAIT(media_image);
		cairo_surface_write_to_png(media_image, ".checks/media-player.png");
		cairo_surface_destroy(media_image);
		/* GTK WebKit's audio Play follows its 15-second-back button. Video accepts
		 * the central play overlay; geometry is in CSS pixels before page zoom. */
		char *point = evaluate(b, video ?
		    "const r=media.getBoundingClientRect(); [r.left+r.width/2,r.top+r.height/2].join(',')" :
		    "const r=media.getBoundingClientRect(); [r.left+64,r.top+r.height/2].join(',')");
		char *end;
		double zoom = webkit_web_view_get_zoom_level(b->view);
		double x = g_ascii_strtod(point, &end) * zoom;
		double y = g_ascii_strtod(end + 1, NULL) * zoom;
		g_free(point);
		send_browser_click(b, x, y);
		WAIT(script_equals(b, "String(!media.paused && media.currentTime > 0.1)", "true"));
		expect_script(b, "String(media.error === null)", "true");
		if (video) {
			expect_script(b, "String(media.videoWidth)", "96");
			WAIT(script_equals(b, "String(media.getVideoPlaybackQuality().totalVideoFrames > 0)", "true"));
		}
		send_browser_click(b, x, y);
		WAIT(script_equals(b, "String(media.paused)", "true"));
		g_assert_cmpstr(page->url, ==, url);
		int count = g_list_length(app->pages);
		app_close(page);
		WAIT(g_list_length(app->pages) == count - 1);
		g_free(path); g_free(url);
	}
	g_free(cwd);
}
