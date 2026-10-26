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
	press_browser_key(page, GDK_KEY_Escape, 0);
	g_assert_false(find_visible(b));
	g_assert_true(gtk_window_get_focus(GTK_WINDOW(page->app->window)) == browser_view(b));
	WAIT(script_equals(b, "document.activeElement.tagName + ':' + getSelection().toString()", "A:Popup"));
	expect_script(b, "document.querySelector('a').addEventListener('click', e => {"
	              "e.preventDefault(); window.findActivated = true; }); 'ok'", "ok");
	GdkEvent *event = gdk_event_new(GDK_KEY_PRESS);
	event->key.window = g_object_ref(gtk_widget_get_window(browser_view(b)));
	event->key.keyval = GDK_KEY_Return;
	gdk_event_set_device(event, gdk_seat_get_keyboard(gdk_display_get_default_seat(
	    gtk_widget_get_display(browser_view(b)))));
	gtk_widget_event(browser_view(b), event);
	event->type = GDK_KEY_RELEASE;
	gtk_widget_event(browser_view(b), event);
	gdk_event_free(event);
	WAIT(script_equals(b, "String(window.findActivated)", "true"));
	gtk_widget_show_all(browser_widget(b));
	g_assert_false(find_visible(b));
	press_browser_key(page, GDK_KEY_f, GDK_CONTROL_MASK);
	WAIT(responses == 6);
	press_browser_key(page, GDK_KEY_k, GDK_CONTROL_MASK);
	g_assert_false(find_visible(b));
	press_browser_key(page, GDK_KEY_Escape, 0);
	press_browser_key(page, GDK_KEY_f, GDK_CONTROL_MASK);
	WAIT(responses == 7);
	press_browser_key(page, GDK_KEY_r, GDK_CONTROL_MASK);
	WAIT(!webkit_web_view_is_loading(b->view));
	g_assert_false(find_visible(b));
	g_signal_handler_disconnect(controller, signal);
}
