/* Shared real-WebKit script assertions and embedded filesystem regressions. */
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
	if (strcmp(value, expected)) g_error("Expected %s; got %s", expected, value);
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
expect_denied(Browser *browser, const char *url)
{
	char *probe = g_strdup_printf("window.denied = false; fetch('%s')"
	    ".then(r => { window.denied = !r.ok; }).catch(() => { window.denied = true; }); 'ok'", url);
	expect_script(browser, probe, "ok");
	g_free(probe);
	WAIT(script_equals(browser, "String(window.denied)", "true"));
}

static void
expect_sort(Browser *browser, const char *key, const char *first_file)
{
	char *script = g_strdup_printf("document.querySelector('[data-sort=%s]').click(); "
	                              "document.links[1].textContent", key);
	expect_script(browser, script, first_file);
	g_free(script);
}

/* Embedded directory browsing runs through the real WebKit scheme handler. */
static void
check_explorer(Browser *browser)
{
	char *cwd = g_get_current_dir();
	char *directory = g_build_filename(cwd, ".checks", "explorer- #?% ü-XXXXXX", NULL);
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
	char *url = g_strconcat(WORKSPACE_BROWSER_HOME "files?path=", escaped, NULL);
	char *saved = g_filename_to_uri(directory, NULL, NULL);
	char *trailing = g_strconcat(saved, "/", NULL);
	const char *targets[] = {saved, trailing, url};
	for (size_t i = 0; i < G_N_ELEMENTS(targets); i++) {
		webkit_web_view_load_uri(browser->view, targets[i]);
		WAIT(!webkit_web_view_is_loading(browser->view));
		WAIT(g_str_has_prefix(webkit_web_view_get_uri(browser->view), "worminal:"));
		WAIT(script_equals(browser, "String(document.links.length)", "3"));
	}
	g_free(saved); g_free(trailing);
	expect_script(browser, "String(document.querySelectorAll('table,dialog').length)", "0");
	expect_script(browser, "getComputedStyle(document.querySelector('#open')).display", "none");
	expect_script(browser, "String(document.querySelectorAll('select').length)", "0");
	expect_script(browser, "document.querySelector('[data-sort=name]').textContent", "Name ↑");
	expect_script(browser, "document.links[0].textContent", "empty");
	expect_script(browser, "getComputedStyle(document.links[0]).textDecorationLine", "none");
	expect_sort(browser, "modified", "recent.z");
	expect_sort(browser, "modified", "literal <name>.txt");
	expect_sort(browser, "size", "literal <name>.txt");
	expect_sort(browser, "size", "recent.z");
	expect_sort(browser, "extension", "literal <name>.txt");
	expect_sort(browser, "extension", "recent.z");
	expect_script(browser, "getComputedStyle(document.documentElement).backgroundColor", "rgb(20, 20, 20)");
	expect_script(browser, "Array.from(document.links).find(a => a.textContent === 'literal <name>.txt').click(); 'ok'",
	              "ok");
	WAIT(script_equals(browser, "document.body.textContent.trim()", "literal <text>"));
	webkit_web_view_go_back(browser->view);
	WAIT(script_equals(browser, "String(document.links.length)", "3"));
	expect_script(browser, "document.querySelector('[data-sort=modified]').click(); "
	              "document.querySelector('[data-sort=extension]').click(); "
	              "Array.from(document.links).find(a => a.textContent === 'empty').click(); 'ok'", "ok");
	WAIT(script_equals(browser, "String(new URLSearchParams(location.search).get('path')?.endsWith('/empty'))", "true"));
	WAIT(!webkit_web_view_is_loading(browser->view));
	WAIT(script_equals(browser, "document.querySelector('#entries').textContent", ""));
	expect_script(browser, "document.querySelector('[data-sort=extension]').textContent", "Extension ↑");
	/* Native errors use the submodule template and insert escaped text only. */
	const char *message = "literal <img onerror=alert(1)> & error";
	error_page(browser->view, "worminal:///missing", message);
	WAIT(script_equals(browser, "document.body.textContent.trim()", message));
	expect_script(browser, "String(document.querySelectorAll('img,script').length)", "0");
	expect_script(browser, "getComputedStyle(document.documentElement).backgroundColor", "rgb(20, 20, 20)");
	/* A different document in the same view must not retain filesystem privileges. */
	webkit_web_view_load_html(browser->view, "<!doctype html><title>Untrusted</title>", "https://example.org/");
	WAIT(!webkit_web_view_is_loading(browser->view));
	expect_denied(browser, "worminal:///list?path=%2F");
	char *file_path = g_uri_escape_string(file, "/", FALSE);
	char *file_url = g_strconcat("worminal://", file_path, NULL);
	expect_denied(browser, file_url);
	webkit_web_view_load_uri(browser->view, file_url);
	WAIT(script_equals(browser, "document.body.textContent.trim()", "literal <text>"));
	g_free(file_path); g_free(file_url);
	file_url = g_filename_to_uri(file, NULL, NULL);
	webkit_web_view_load_uri(browser->view, file_url);
	WAIT(script_equals(browser, "document.body.textContent.trim()", "literal <text>"));
	g_assert_cmpstr(webkit_web_view_get_uri(browser->view), ==, file_url);
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
	char *file_document = g_build_filename(file_cwd, ".checks", "preview-resource.html", NULL);
	g_assert_true(g_file_set_contents(file_document, "<!doctype html><title>Local resource</title><input>", -1, NULL));
	char *file_url = g_filename_to_uri(file_document, NULL, NULL);
	Page *home = open_browser(app, file_url);
	g_free(file_url); g_free(file_cwd);
	char *home_url = g_strdup(webkit_web_view_get_uri(home->browser->view));
	g_assert_true(related_view(home_url) == home->browser->view);
	g_assert_null(related_view("file:///etc/hosts"));
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
	int home_count = g_list_length(app->pages);
	app_close(home);
	WAIT(g_list_length(app->pages) == home_count - 1);
	g_unlink(file_document); g_free(file_document);
}
