/* Shared real-WebKit script assertions and embedded filesystem regressions. */
static GtkWidget *dialog_of_type(GType type);

static char *script_result;
static gboolean script_done;
static void
script_finished(GObject *source, GAsyncResult *result, gpointer data)
{
	GError *error = NULL;
	JSCValue *value = webkit_web_view_evaluate_javascript_finish(WEBKIT_WEB_VIEW(source), result, &error);
	if (error) g_error("JavaScript %s: %s", (const char *)data, error->message);
	script_result = jsc_value_to_string(value);
	g_object_unref(value);
	script_done = TRUE;
}

static char *
evaluate(Browser *b, const char *script)
{
	script_done = FALSE;
	webkit_web_view_evaluate_javascript(b->view, script, -1, NULL, NULL, NULL, script_finished, (gpointer)script);
	WAIT(script_done);
	return script_result;
}

static void
expect_script(Browser *b, const char *script, const char *expected)
{
	char *value = evaluate(b, script);
	if (strcmp(value, expected)) {
		char *body = evaluate(b, "document.body.textContent.slice(0, 300)");
		g_error("Expected %s; got %s at %s; page: %s", expected, value,
		    webkit_web_view_get_uri(b->view), body);
	}
	g_free(value);
}

static gboolean
script_equals(Browser *b, const char *script, const char *expected)
{
	char *value = evaluate(b, script);
	gboolean equal = !strcmp(value, expected);
	g_free(value);
	return equal;
}

static void
send_widget_key(GtkWidget *widget, guint key, guint mods)
{
	GdkEvent *event = gdk_event_new(GDK_KEY_PRESS);
	event->key.window = g_object_ref(gtk_widget_get_window(widget));
	event->key.keyval = key;
	event->key.state = mods;
	/* GTK entry bindings translate hardware codes, unlike WebKit's keyval path. */
	GdkKeymapKey *keys = NULL;
	int count = 0;
	if (gdk_keymap_get_entries_for_keyval(gdk_keymap_get_for_display(
	    gtk_widget_get_display(widget)), key, &keys, &count) && count) {
		event->key.hardware_keycode = keys[0].keycode;
		event->key.group = keys[0].group;
	}
	g_free(keys);
	gdk_event_set_device(event, gdk_seat_get_keyboard(gdk_display_get_default_seat(
	    gtk_widget_get_display(widget))));
	gtk_widget_event(widget, event);
	event->type = GDK_KEY_RELEASE;
	gtk_widget_event(widget, event);
	gdk_event_free(event);
}

static void
send_browser_key(Browser *b, guint key, guint mods)
{
	send_widget_key(browser_view(b), key, mods);
}

static void
send_browser_click(Browser *b, double x, double y)
{
	GdkDevice *pointer = gdk_seat_get_pointer(gdk_display_get_default_seat(gtk_widget_get_display(browser_view(b))));
	GdkEvent *motion = gdk_event_new(GDK_MOTION_NOTIFY);
	motion->motion.window = g_object_ref(gtk_widget_get_window(browser_view(b)));
	motion->motion.x = x; motion->motion.y = y;
	gdk_event_set_device(motion, pointer);
	gtk_widget_event(browser_view(b), motion);
	gdk_event_free(motion);
	GdkEvent *event = gdk_event_new(GDK_BUTTON_PRESS);
	event->button.window = g_object_ref(gtk_widget_get_window(browser_view(b)));
	event->button.button = 1;
	event->button.x = x; event->button.y = y;
	event->button.time = (guint32)(g_get_monotonic_time() / 1000);
	gdk_event_set_device(event, pointer);
	gtk_widget_event(browser_view(b), event);
	event->type = GDK_BUTTON_RELEASE;
	event->button.state = GDK_BUTTON1_MASK;
	gtk_widget_event(browser_view(b), event);
	gdk_event_free(event);
}

static void
expect_denied(Browser *browser, const char *url)
{
	char *probe = g_strdup_printf("window.denied = false; fetch('%s')"
	    ".then(r => { window.denied = !r.ok; }).catch(() => { window.denied = true; }); 'ok'", url);
	expect_script(browser, probe, "ok");
	g_free(probe);
	WAIT(script_equals(browser, "String(window.denied)", "true"));
}

static void
expect_file_denied(Browser *browser, const char *path)
{
	char *escaped = g_strescape(path, NULL);
	char *script = g_strdup_printf("window.badFile='pending'; window.worminal.readFile('%s')"
	    ".then(() => window.badFile='allowed', () => window.badFile='denied'); 'asked'", escaped);
	expect_script(browser, script, "asked");
	WAIT(script_equals(browser, "window.badFile", "denied"));
	g_free(script); g_free(escaped);
}

static void
expect_sort(Browser *browser, const char *key, const char *first_file)
{
	char *script = g_strdup_printf("document.querySelector('[data-sort=%s]').click(); "
	                              "document.querySelectorAll('#entries .entry-name')[1].textContent", key);
	expect_script(browser, script, first_file);
	g_free(script);
}

/* Embedded directory browsing runs through the real WebKit scheme handler. */
static void
check_explorer(Browser *browser)
{
	char *cwd = g_get_current_dir();
	char *directory = g_build_filename(cwd, ".checks", "explorer- +#?% ü-XXXXXX", NULL);
	g_assert_nonnull(g_mkdtemp(directory));
	char *empty = g_build_filename(directory, "empty", NULL);
	char *file = g_build_filename(directory, "literal <name>.txt", NULL);
	char *recent = g_build_filename(directory, "recent.z", NULL);
	g_assert_cmpint(g_mkdir(empty, 0700), ==, 0);
	g_assert_true(g_file_set_contents(file, "literal <text>", -1, NULL));
	g_assert_true(g_file_set_contents(recent, "new", -1, NULL));
	GFile *old_file = g_file_new_for_path(file), *new_file = g_file_new_for_path(recent);
	g_assert_true(g_file_set_attribute_uint64(old_file, G_FILE_ATTRIBUTE_TIME_MODIFIED,
	                                       1000, G_FILE_QUERY_INFO_NONE, NULL, NULL));
	g_assert_true(g_file_set_attribute_uint64(new_file, G_FILE_ATTRIBUTE_TIME_MODIFIED,
	                                       2000, G_FILE_QUERY_INFO_NONE, NULL, NULL));
	g_object_unref(old_file); g_object_unref(new_file);
	char *escaped = g_uri_escape_string(directory, NULL, FALSE);
	char *url = g_strconcat(g_getenv("WORMINAL_PREVIEW_LOCAL_APP"), "?path=", escaped, NULL);
	const char *targets[] = {url};
	for (size_t i = 0; i < G_N_ELEMENTS(targets); i++) {
		webkit_web_view_load_uri(browser->view, targets[i]);
		WAIT(!webkit_web_view_is_loading(browser->view));
		WAIT(g_str_has_prefix(webkit_web_view_get_uri(browser->view), "file:"));
		expect_script(browser, "typeof window.worminal", "object");
		WAIT(script_equals(browser, "String(Boolean(document.querySelector('#entries')?.textContent))", "true"));
		expect_script(browser, "String(document.querySelectorAll('.entry-name').length)", "3");
	}
	/* Downloadable files must not replace the directory with an error page. */
	expect_script(browser, "Array.from(document.querySelectorAll('#entries .entry-name'))"
	              ".find(a => a.textContent === 'recent.z').parentElement"
	              ".querySelector('.open-file').click(); 'ok'", "ok");
	WAIT(dialog_of_type(GTK_TYPE_FILE_CHOOSER_DIALOG));
	gtk_dialog_response(GTK_DIALOG(dialog_of_type(GTK_TYPE_FILE_CHOOSER_DIALOG)), GTK_RESPONSE_CANCEL);
	WAIT(!webkit_web_view_is_loading(browser->view));
	g_assert_cmpstr(webkit_web_view_get_uri(browser->view), ==, url);
	expect_script(browser, "String(document.querySelectorAll('.entry-name').length)", "3");
	/* A file can disappear between listing and activation; Back must keep the listing. */
	g_assert_cmpint(g_unlink(file), ==, 0);
	expect_script(browser, "Array.from(document.querySelectorAll('#entries .entry-name'))"
	              ".find(a => a.textContent === 'literal <name>.txt').parentElement"
	              ".querySelector('.open-file').click(); 'ok'", "ok");
	WAIT(script_equals(browser, "String(document.querySelector('#entries') === null)", "true"));
	g_assert_true(webkit_web_view_can_go_back(browser->view));
	g_assert_true(browser_shortcut(browser->page, GDK_KEY_Left, GDK_MOD1_MASK));
	WAIT(script_equals(browser, "String(document.querySelectorAll('.entry-name').length)", "3"));
	g_assert_true(g_file_set_contents(file, "literal <text>", -1, NULL));

	expect_script(browser, "String(document.querySelectorAll('table,dialog').length)", "0");
	expect_script(browser, "getComputedStyle(document.querySelector('#open')).display", "none");
	expect_script(browser, "String(document.querySelectorAll('select').length)", "0");
	expect_script(browser, "document.querySelector('[data-sort=modified]').textContent", "Modified ↓");
	expect_script(browser, "document.querySelectorAll('#entries .entry-name')[0].textContent", "empty");
	expect_script(browser, "getComputedStyle(document.querySelector('.entry-name')).textDecorationLine", "none");
	expect_sort(browser, "modified", "literal <name>.txt");
	expect_sort(browser, "modified", "recent.z");
	expect_sort(browser, "size", "literal <name>.txt");
	expect_sort(browser, "size", "recent.z");
	expect_sort(browser, "extension", "literal <name>.txt");
	expect_sort(browser, "extension", "recent.z");
	expect_script(browser, "getComputedStyle(document.documentElement).backgroundColor", "rgb(20, 20, 20)");
	expect_script(browser, "Array.from(document.querySelectorAll('#entries .entry-name'))"
	              ".find(a => a.textContent === 'literal <name>.txt').parentElement"
	              ".querySelector('.open-file').click(); 'ok'",
	              "ok");
	WAIT(script_equals(browser, "document.body.textContent.trim()", "literal <text>"));
	webkit_web_view_go_back(browser->view);
	WAIT(script_equals(browser, "String(document.querySelectorAll('.entry-name').length)", "3"));
	expect_script(browser, "document.querySelector('[data-sort=modified]').click(); "
	              "document.querySelector('[data-sort=extension]').click(); "
	              "Array.from(document.querySelectorAll('#entries .entry-name'))"
	              ".find(a => a.textContent === 'empty').click(); 'ok'", "ok");
	WAIT(script_equals(browser, "String(new URLSearchParams(location.search).get('path')?.endsWith('/empty'))", "true"));
	WAIT(!webkit_web_view_is_loading(browser->view));
	WAIT(script_equals(browser, "document.querySelector('#entries').textContent", ""));
	expect_script(browser, "document.querySelector('[data-sort=extension]').textContent", "Extension ↑");
	/* Exercise the real file bridge, worker, and canvas in WebKit, not just PDF parsing in Node. */
	char *pdf_source = g_build_filename(cwd, "file-explorer", "tests", "fixtures", "preview.pdf", NULL);
	char *pdf_path = g_build_filename(empty, "preview.pdf", NULL);
	char *pdf_bytes = NULL;
	gsize pdf_size = 0;
	g_assert_true(g_file_get_contents(pdf_source, &pdf_bytes, &pdf_size, NULL));
	g_assert_true(g_file_set_contents(pdf_path, pdf_bytes, pdf_size, NULL));
	g_free(pdf_source); g_free(pdf_bytes);
	webkit_web_view_reload(browser->view);
	WAIT(script_equals(browser, "document.querySelector('.entry-name')?.textContent", "preview.pdf"));
	expect_script(browser, "document.querySelector('.entry-name').dispatchEvent(new Event('pointerenter')); 'ok'", "ok");
	WAIT(script_equals(browser, "String(document.querySelector('canvas.image-preview')?.hidden)", "false"));
	expect_script(browser, "Array.from(document.querySelector('canvas').getContext('2d')"
	              ".getImageData(50,50,1,1).data).join(',')", "255,0,0,255");
	expect_script(browser, "document.querySelector('.entry-name').dispatchEvent(new Event('pointerleave')); 'ok'", "ok");
	expect_script(browser, "String(document.querySelector('.image-preview'))", "null");
	g_unlink(pdf_path); g_free(pdf_path);
	char *video_source = g_build_filename(cwd, "tests", "fixtures", "media", "sample.mp4", NULL);
	char *video_path = g_build_filename(empty, "preview.mp4", NULL);
	char *video_bytes = NULL;
	gsize video_size = 0;
	g_assert_true(g_file_get_contents(video_source, &video_bytes, &video_size, NULL));
	g_assert_true(g_file_set_contents(video_path, video_bytes, video_size, NULL));
	g_free(video_source); g_free(video_bytes);
	webkit_web_view_reload(browser->view);
	WAIT(script_equals(browser, "document.querySelector('.entry-name')?.textContent", "preview.mp4"));
	expect_script(browser, "document.querySelector('.entry-name').dispatchEvent(new Event('pointerenter')); 'ok'", "ok");
	WAIT(script_equals(browser, "String(document.querySelector('video.image-preview')?.hidden)", "false"));
	expect_script(browser, "String(document.querySelector('video').muted)", "true");
	expect_script(browser, "String(document.querySelector('video').videoWidth)", "96");
	expect_script(browser, "document.querySelector('.entry-name').dispatchEvent(new Event('pointerleave')); 'ok'", "ok");
	expect_script(browser, "String(document.querySelector('.image-preview'))", "null");
	g_unlink(video_path); g_free(video_path);

	/* Native errors use the submodule template and insert escaped text only. */
	const char *message = "literal <img onerror=alert(1)> & error";
	error_page(browser->view, "worminal:///missing", message);
	WAIT(script_equals(browser, "document.body.textContent.trim()", message));
	expect_script(browser, "String(document.querySelectorAll('img,script').length)", "0");
	expect_script(browser, "getComputedStyle(document.documentElement).backgroundColor", "rgb(24, 24, 24)");
	/* A different document in the same view must not retain filesystem privileges. */
	webkit_web_view_load_html(browser->view, "<!doctype html><title>Untrusted</title>", "https://example.org/");
	WAIT(!webkit_web_view_is_loading(browser->view));
	expect_denied(browser, "worminal:///list?path=%2F");
	char *file_path = g_uri_escape_string(file, "/", FALSE);
	char *file_url = g_strconcat("worminal://", file_path, NULL);
	expect_denied(browser, file_url);
	g_free(file_path); g_free(file_url);
	file_url = g_filename_to_uri(file, NULL, NULL);
	webkit_web_view_load_uri(browser->view, file_url);
	WAIT(script_equals(browser, "document.body.textContent.trim()", "literal <text>"));
	g_assert_cmpstr(webkit_web_view_get_uri(browser->view), ==, file_url);
	expect_denied(browser, "worminal:///list?path=%2F");
	g_free(file_url);
	g_unlink(recent); g_free(recent);
	g_unlink(file);
	g_rmdir(empty);
	g_rmdir(directory);
	g_free(url); g_free(escaped); g_free(file); g_free(empty); g_free(directory); g_free(cwd);
}

static Page *open_browser(App *app, const char *url);

static void
check_file_views(App *app)
{
	/* File views retain their own tab storage while reusing the same resource's
	 * renderer. Closing the original view must leave its sibling usable. */
	char *file_cwd = g_get_current_dir();
	char *file_document = g_build_filename(file_cwd, ".checks", "preview-resource #?% ü.html", NULL);
	g_assert_true(g_file_set_contents(file_document, "<!doctype html><title>Local resource</title><input>"
	    "<script>window.documentVersion='first';"
	    "if(parent!==window)parent.postMessage(typeof window.worminal,'*')</script>", -1, NULL));
	char *file_url = g_filename_to_uri(file_document, NULL, NULL);
	Page *home = open_browser(app, file_url);
	g_assert_cmpstr(gtk_label_get_text(GTK_LABEL(home->label)), ==, "preview-resource #?% ü.html");
	g_free(file_url); g_free(file_cwd);
	char *home_url = g_strdup(webkit_web_view_get_uri(home->browser->view));
	g_assert_true(related_view(home_url) == home->browser->view);
	g_assert_null(related_view("file:///etc/hosts"));
	/* A selected local HTML app gets the generic metadata API, including query routes. */
	char *route = g_strconcat(home_url, "?route=two", NULL);
	webkit_web_view_load_uri(home->browser->view, route);
	WAIT(!webkit_web_view_is_loading(home->browser->view));
	g_free(route);
	g_assert_cmpstr(gtk_label_get_text(GTK_LABEL(home->label)), ==, "preview-resource #?% ü.html");
	expect_script(home->browser, "window.worminal.readDirectory('/')"
	              ".then(rows => window.localEntryCount = rows.length); 'asked'", "asked");
	WAIT(script_equals(home->browser, "typeof window.localEntryCount", "number"));
	expect_script(home->browser, "window.worminal.readFile(decodeURIComponent(location.pathname))"
	              ".then(bytes => window.localFileText = new TextDecoder().decode(bytes)); 'asked'", "asked");
	WAIT(script_equals(home->browser, "String(window.localFileText?.startsWith('<!doctype html>'))", "true"));
	expect_file_denied(home->browser, "/dev/zero");
	expect_file_denied(home->browser, "/");
	expect_file_denied(home->browser, "relative.pdf");
	char *large = g_strconcat(file_document, ".large", NULL);
	int large_fd = open(large, O_CREAT | O_WRONLY | O_EXCL, 0600);
	g_assert_cmpint(large_fd, >=, 0);
	g_assert_cmpint(ftruncate(large_fd, 32 * 1024 * 1024 + 1), ==, 0);
	close(large_fd);
	expect_file_denied(home->browser, large);
	g_unlink(large); g_free(large);
	char *fifo = g_strconcat(file_document, ".fifo", NULL);
	g_assert_cmpint(mkfifo(fifo, 0600), ==, 0);
	expect_file_denied(home->browser, fifo);
	g_unlink(fifo); g_free(fifo);
	expect_denied(home->browser, "worminal:///file?path=%2Fetc%2Fhosts");
	expect_denied(home->browser, "worminal:///list?path=%2F");
	expect_script(home->browser, "window.childBridge='pending'; "
	              "addEventListener('message', e => window.childBridge=e.data);"
	              "const frame=document.createElement('iframe'); frame.setAttribute('sandbox','allow-scripts');"
	              "frame.src=location.href; document.body.append(frame); 'created'", "created");
	WAIT(script_equals(home->browser, "window.childBridge", "undefined"));


	expect_script(home->browser, "sessionStorage.setItem('file-tab', 'first'); 'ok'", "ok");
	Page *second_home = open_browser(app, home_url);
	expect_script(second_home->browser, "String(window.opener)", "null");
	expect_script(second_home->browser, "String(sessionStorage.getItem('file-tab'))", "null");
	int original_home_count = g_list_length(app->pages);
	app_close(home);
	WAIT(g_list_length(app->pages) == original_home_count - 1);
	home = second_home;
	g_free(home_url);
	expect_script(home->browser, "sessionStorage.setItem('surviving-tab', 'yes'); "
	              "sessionStorage.getItem('surviving-tab')", "yes");
	/* Replacing the on-disk app needs neither a native rebuild nor a new owner process. */
	g_assert_true(g_file_set_contents(file_document, "<!doctype html><title>Updated local resource</title>"
	    "<style>body{color:rgb(1,2,3)}</style><script>window.documentVersion='second'</script>", -1, NULL));
	/* Disk replacement is explicit: the existing page stays until normal Ctrl+R. */
	expect_script(home->browser, "window.documentVersion", "first");
	g_assert_true(browser_shortcut(home, GDK_KEY_r, GDK_CONTROL_MASK));
	WAIT(script_equals(home->browser, "window.documentVersion", "second"));
	expect_script(home->browser, "getComputedStyle(document.body).color", "rgb(1, 2, 3)");
	expect_script(home->browser, "sessionStorage.getItem('surviving-tab')", "yes");
	Page *latest = open_browser(app, home->url);
	WAIT(!g_strcmp0(webkit_web_view_get_title(latest->browser->view), "Updated local resource"));
	expect_script(latest->browser, "window.documentVersion", "second");
	g_assert_cmpstr(gtk_label_get_text(GTK_LABEL(latest->label)), ==, "preview-resource #?% ü.html");
	expect_script(latest->browser, "getComputedStyle(document.body).color", "rgb(1, 2, 3)");
	/* A URI change must update the label even when the document title stays unchanged. */
	char *parent = g_path_get_dirname(file_document);
	char *other_path = g_build_filename(parent, "other #file.html", NULL);
	g_assert_true(g_file_set_contents(other_path, "<!doctype html><title>Updated local resource</title>", -1, NULL));
	char *other_url = g_filename_to_uri(other_path, NULL, NULL);
	webkit_web_view_load_uri(latest->browser->view, other_url);
	WAIT(!g_strcmp0(webkit_web_view_get_uri(latest->browser->view), other_url));
	WAIT(!webkit_web_view_is_loading(latest->browser->view));
	g_assert_cmpstr(gtk_label_get_text(GTK_LABEL(latest->label)), ==, "other #file.html");
	g_unlink(other_path);
	g_free(parent); g_free(other_path); g_free(other_url);
	int latest_count = g_list_length(app->pages);
	app_close(latest);
	WAIT(g_list_length(app->pages) == latest_count - 1);
	int home_count = g_list_length(app->pages);
	app_close(home);
	WAIT(g_list_length(app->pages) == home_count - 1);
	g_unlink(file_document); g_free(file_document);
}
