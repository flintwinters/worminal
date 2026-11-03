/* Select retained output through the real shortcut and daemon transport. */
static void
check_select_all(App *app, Terminal *t)
{
	GString *output = g_string_new("\r\nHISTORY-START\r\n");
	for (int row = 0; row < 1100; row++)
		g_string_append_printf(output, "row-%04d retained terminal output abcdefghijklmnopqrstuvwxyz\r\n", row);
	g_string_append(output, "HISTORY-END");
	input(t, output->str, output->len);
	g_string_free(output, TRUE);
	gint64 deadline = g_get_monotonic_time() + 10000000;
	while (!screen_has(t, "HISTORY-END") && g_get_monotonic_time() < deadline) {
		while (g_main_context_iteration(NULL, FALSE)) {}
		g_usleep(1000);
	}
	g_assert_true(screen_has(t, "HISTORY-END"));
	g_assert_false(screen_has(t, "HISTORY-START"));
	GdkEventKey key = {.type = GDK_KEY_PRESS, .keyval = GDK_KEY_a, .state = GDK_CONTROL_MASK};
	g_assert_true(shortcut(app->window, &key, app));
	/* Copy immediately, including while the multi-packet response is pending. */
	key.keyval = GDK_KEY_c;
	key.state |= GDK_SHIFT_MASK;
	g_assert_true(shortcut(app->window, &key, app));
	deadline = g_get_monotonic_time() + 10000000;
	while (t->all_pending && g_get_monotonic_time() < deadline) {
		while (g_main_context_iteration(NULL, FALSE)) {}
		g_usleep(1000);
	}
	g_assert_false(t->all_pending);
	char *copied = gtk_clipboard_wait_for_text(gtk_clipboard_get(GDK_SELECTION_CLIPBOARD));
	g_assert_nonnull(copied);
	g_assert_nonnull(strstr(copied, "HISTORY-START"));
	g_assert_nonnull(strstr(copied, "row-0500 retained terminal output"));
	g_assert_nonnull(strstr(copied, "HISTORY-END"));
	g_assert_cmpstr(copied, ==, t->all_text->str);
	g_free(copied);
	tsessionuse(t->session);
	g_assert_true(selected(0, 0));
	g_assert_true(selected(t->frame.cols - 1, t->frame.rows - 1));
	/* Input cancels a cached full-buffer selection. */
	input(t, "!", 1);
	g_assert_null(t->all_text);
}
