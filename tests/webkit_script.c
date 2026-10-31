/* Shared real-WebKit JavaScript evaluation; the including check supplies WAIT. */
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
evaluate_view(WebKitWebView *view, const char *script)
{
	script_done = FALSE;
	webkit_web_view_evaluate_javascript(view, script, -1, NULL, NULL, NULL, script_finished, (gpointer)script);
	WAIT(script_done);
	return script_result;
}


static gboolean
script_equals_view(WebKitWebView *view, const char *script, const char *expected)
{
	char *value = evaluate_view(view, script);
	gboolean equal = !strcmp(value, expected);
	g_free(value);
	return equal;
}
