/* Creation follows the selected tab; the explicit terminal shortcut remains available. */
static void
check_new_tab_shortcuts(App *app, uint32_t browser_id)
{
	GdkEventKey create = {.type = GDK_KEY_PRESS, .keyval = GDK_KEY_t, .state = GDK_CONTROL_MASK};
	g_assert_true(shortcut(app->window, &create, app));
	WAIT(app->active && app->active->browser && app->active->id != browser_id);
	g_assert_null(app->active->terminal);
	g_assert_cmpstr(app->active->url, ==, WORKSPACE_BROWSER_HOME);
	uint32_t blank_id = app->active->id;
	app_close(app->active);
	WAIT(!find_page(app, blank_id) && app->active && app->active->id == browser_id);
	create.state |= GDK_SHIFT_MASK;
	g_assert_true(shortcut(app->window, &create, app));
	WAIT(app->active && app->active->terminal && app->active->id != browser_id);
	uint32_t terminal_id = app->active->id;
	create.state = GDK_CONTROL_MASK;
	g_assert_true(shortcut(app->window, &create, app));
	WAIT(app->active && app->active->terminal && app->active->id != terminal_id);
	uint32_t extra_id = app->active->id;
	app_close(app->active);
	WAIT(!find_page(app, extra_id) && app->active && app->active->id == terminal_id);
}
