/* WebKit owns matching; an isolated-world helper presents and accepts the selected result. */
static void
find_found(WebKitFindController *controller, guint matches, guint *responses)
{
	(void)controller;
	g_assert_cmpuint(matches, >, 0);
	(*responses)++;
}

static void
check_browser_find(Page *page)
{
	Browser *b = page->browser;
	g_assert_null(b->find);
	press_browser_key(page, GDK_KEY_f, GDK_CONTROL_MASK);
	g_assert_true(find_visible(b));
	g_assert_true(gtk_window_get_focus(GTK_WINDOW(page->app->window)) == b->find);
	WebKitFindController *controller = webkit_web_view_get_find_controller(b->view);
	guint responses = 0;
	gulong signal = g_signal_connect(controller, "found-text", G_CALLBACK(find_found), &responses);
	gtk_entry_set_text(GTK_ENTRY(b->find), "popup");
	WAIT(responses == 1);
	WAIT(script_equals(b, "getComputedStyle(document.body, '::selection').backgroundColor", "rgb(255, 221, 0)"));
	g_assert_cmpstr(webkit_find_controller_get_search_text(controller), ==, "popup");
	press_browser_key(page, GDK_KEY_f, GDK_CONTROL_MASK);
	int start, end;
	g_assert_true(gtk_editable_get_selection_bounds(GTK_EDITABLE(b->find), &start, &end));
	g_assert_cmpint(start, ==, 0);
	g_assert_cmpint(end, ==, 5);
	WAIT(responses == 2);
	gtk_widget_grab_focus(browser_view(b));
	g_assert_false(browser_shortcut(page, GDK_KEY_Return, 0));
	gtk_widget_grab_focus(b->find);
	/* The fixture has one Popup link: both directions must wrap to it. */
	press_browser_key(page, GDK_KEY_Return, 0);
	WAIT(responses == 3);
	press_browser_key(page, GDK_KEY_Return, GDK_SHIFT_MASK);
	WAIT(responses == 4);
	gtk_entry_set_text(GTK_ENTRY(b->find), "absent-find-fixture");
	WAIT(gtk_style_context_has_class(gtk_widget_get_style_context(b->find), "error"));
	gtk_entry_set_text(GTK_ENTRY(b->find), "");
	g_assert_false(gtk_style_context_has_class(gtk_widget_get_style_context(b->find), "error"));
	gtk_entry_set_text(GTK_ENTRY(b->find), "popup");
	WAIT(responses == 5);
	send_widget_key(page->app->window, GDK_KEY_Escape, 0);
	g_assert_false(find_visible(b));
	g_assert_true(gtk_window_get_focus(GTK_WINDOW(page->app->window)) == browser_view(b));
	WAIT(script_equals(b, "document.activeElement.tagName + ':' + getSelection().toString()", "A:Popup"));
	expect_script(b, "document.querySelector('a').addEventListener('click', e => {"
	              "e.preventDefault(); window.findActivated = true; }, {once:true}); 'ok'", "ok");
	send_browser_key(b, GDK_KEY_Return, 0);
	WAIT(script_equals(b, "String(window.findActivated)", "true"));
	gtk_widget_show_all(browser_widget(b));
	g_assert_false(find_visible(b));
	send_widget_key(page->app->window, GDK_KEY_f, GDK_CONTROL_MASK);
	WAIT(responses == 6);
	g_assert_true(gtk_window_get_focus(GTK_WINDOW(page->app->window)) == b->find);
	g_assert_true(gtk_editable_get_selection_bounds(GTK_EDITABLE(b->find), &start, &end));
	g_assert_cmpint(start, ==, 0);
	g_assert_cmpint(end, ==, 5);
	/* Dispatch through the window so normal entry editing must replace the query. */
	send_widget_key(page->app->window, GDK_KEY_x, 0);
	g_assert_cmpstr(gtk_entry_get_text(GTK_ENTRY(b->find)), ==, "x");
	send_widget_key(page->app->window, GDK_KEY_f, GDK_CONTROL_MASK);
	g_assert_true(gtk_editable_get_selection_bounds(GTK_EDITABLE(b->find), &start, &end));
	g_assert_cmpint(end, ==, 1);
	send_widget_key(page->app->window, GDK_KEY_BackSpace, 0);
	g_assert_cmpstr(gtk_entry_get_text(GTK_ENTRY(b->find)), ==, "");
	gtk_entry_set_text(GTK_ENTRY(b->find), "popup");
	WAIT(responses == 7);
	press_browser_key(page, GDK_KEY_k, GDK_CONTROL_MASK);
	g_assert_false(find_visible(b));
	press_browser_key(page, GDK_KEY_Escape, 0);
	press_browser_key(page, GDK_KEY_f, GDK_CONTROL_MASK);
	WAIT(responses == 8);
	/* Also follow the accepted link in this tab, rather than intercepting activation. */
	char *original = g_strdup(webkit_web_view_get_uri(b->view));
	expect_script(b, "document.querySelector('a').target = '_self'; 'ok'", "ok");
	send_widget_key(page->app->window, GDK_KEY_Escape, 0);
	WAIT(script_equals(b, "document.activeElement.tagName", "A"));
	send_browser_key(b, GDK_KEY_Return, 0);
	WAIT(g_str_has_suffix(webkit_web_view_get_uri(b->view), "/other") &&
	    !webkit_web_view_is_loading(b->view));
	send_widget_key(page->app->window, GDK_KEY_f, GDK_CONTROL_MASK);
	WAIT(responses == 9);
	send_widget_key(page->app->window, GDK_KEY_x, 0);
	g_assert_cmpstr(gtk_entry_get_text(GTK_ENTRY(b->find)), ==, "x");
	webkit_web_view_load_uri(b->view, original);
	WAIT(!g_strcmp0(webkit_web_view_get_uri(b->view), original) && !webkit_web_view_is_loading(b->view));
	g_free(original);
	press_browser_key(page, GDK_KEY_r, GDK_CONTROL_MASK);
	WAIT(!webkit_web_view_is_loading(b->view));
	g_assert_false(find_visible(b));
	g_signal_handler_disconnect(controller, signal);
}
