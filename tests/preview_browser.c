/* Shared real-WebKit script assertions and embedded filesystem regressions. */
static char *script_result;
static gboolean script_done;
static void
script_finished(GObject *source, GAsyncResult *result, gpointer data)
{
	(void)data;
	GError *error = NULL;
	JSCValue *value = webkit_web_view_evaluate_javascript_finish(WEBKIT_WEB_VIEW(source), result, &error);
	if (error) g_error("JavaScript: %s", error->message);
	script_result = jsc_value_to_string(value);
	g_object_unref(value);
	script_done = TRUE;
}

static char *
evaluate(Browser *b, const char *script)
{
	script_done = FALSE;
	webkit_web_view_evaluate_javascript(b->view, script, -1, NULL, NULL, NULL, script_finished, NULL);
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

/* Embedded directory browsing runs through the real WebKit scheme handler. */
static void
check_explorer(Browser *browser)
{
	char *cwd = g_get_current_dir();
	char *directory = g_build_filename(cwd, ".checks", "explorer- #?% ü-XXXXXX", NULL);
	g_assert_nonnull(g_mkdtemp(directory));
	char *empty = g_build_filename(directory, "empty", NULL);
	char *file = g_build_filename(directory, "literal <name>.txt", NULL);
	g_assert_cmpint(g_mkdir(empty, 0700), ==, 0);
	g_assert_true(g_file_set_contents(file, "literal <text>", -1, NULL));
	char *escaped = g_uri_escape_string(directory, NULL, FALSE);
	char *url = g_strconcat(WORKSPACE_BROWSER_HOME "files?path=", escaped, NULL);
	webkit_web_view_load_uri(browser->view, url);
	WAIT(!webkit_web_view_is_loading(browser->view));
	WAIT(script_equals(browser, "String(document.querySelectorAll('#entries tr').length)", "2"));
	expect_script(browser, "document.querySelector('#entries button').textContent", "empty/");
	expect_script(browser, "document.querySelectorAll('#entries button')[1].click(); 'ok'", "ok");
	WAIT(script_equals(browser, "document.querySelector('#preview-text').textContent", "literal <text>"));
	expect_script(browser, "document.querySelector('#preview').close(); document.querySelector('#open').click(); "
	              "String(document.querySelector('input[aria-label=\"Folder path\"]').hidden)", "false");
	expect_script(browser, "(() => { const p = document.querySelector('input[aria-label=\"Folder path\"]'); "
	              "p.value += '/empty'; p.dispatchEvent(new KeyboardEvent('keydown', {key:'Enter'})); return 'ok'; })()",
	              "ok");
	WAIT(script_equals(browser, "String(document.querySelectorAll('#entries tr').length)", "0"));
	expect_script(browser, "document.querySelector('#notice').textContent", "No matching files.");
	/* A different document in the same view must not retain filesystem privileges. */
	webkit_web_view_load_html(browser->view, "<!doctype html><title>Untrusted</title>", "https://example.org/");
	WAIT(!webkit_web_view_is_loading(browser->view));
	char *probe = g_strdup("window.denied = false; fetch('worminal:///list?path=%2F')"
	                             ".then(r => { window.denied = !r.ok; }).catch(() => { window.denied = true; }); 'ok'");
	expect_script(browser, probe, "ok");
	g_free(probe);
	WAIT(script_equals(browser, "String(window.denied)", "true"));
	char *file_path = g_uri_escape_string(file, "/", FALSE);
	char *file_url = g_strconcat("worminal://", file_path, NULL);
	webkit_web_view_load_uri(browser->view, file_url);
	WAIT(script_equals(browser, "document.body.textContent.trim()", "literal <text>"));
	g_free(file_path); g_free(file_url);
	g_unlink(file);
	g_rmdir(empty);
	g_rmdir(directory);
	g_free(url); g_free(escaped); g_free(file); g_free(empty); g_free(directory); g_free(cwd);
}
