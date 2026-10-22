/* Shared real-WebKit script assertions and native directory navigation regressions. */
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

/* Our plain directory HTML uses ordinary links and browser navigation. */
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
	char *encoded = g_uri_escape_string(directory, "/", FALSE);
	char *url = g_strconcat("worminal://", encoded, NULL);
	webkit_web_view_load_uri(browser->view, url);
	WAIT(!webkit_web_view_is_loading(browser->view));
	WAIT(script_equals(browser, "String(document.body.textContent.includes('literal <name>.txt'))", "true"));
	expect_script(browser, "String(document.querySelectorAll('input,button,script').length)", "0");
	expect_script(browser, "Array.from(document.links).find(a => a.textContent.includes('literal <name>.txt')).click(); 'ok'",
	              "ok");
	WAIT(!webkit_web_view_is_loading(browser->view));
	WAIT(script_equals(browser, "document.body.textContent.trim()", "literal <text>"));
	webkit_web_view_go_back(browser->view);
	WAIT(!webkit_web_view_is_loading(browser->view));
	WAIT(script_equals(browser, "String(document.body.textContent.includes('empty'))", "true"));
	expect_script(browser, "Array.from(document.links).find(a => a.textContent.includes('empty')).click(); 'ok'", "ok");
	WAIT(!webkit_web_view_is_loading(browser->view));
	char *empty_url = g_strconcat(url, "/empty", NULL);
	WAIT(g_str_has_prefix(webkit_web_view_get_uri(browser->view), empty_url));
	g_unlink(file); g_rmdir(empty); g_rmdir(directory);
	g_free(encoded); g_free(empty_url); g_free(url); g_free(file); g_free(empty); g_free(directory); g_free(cwd);
}
