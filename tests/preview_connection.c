/* A socket to the SSH process is not evidence that a remote workspace is ready. */
static void
check_remote_pending_status(void)
{
	App *app = new_app();
	app->master = g_strdup("waiting-test");
	app->window = gtk_window_new(GTK_WINDOW_TOPLEVEL);
	app->status = gtk_label_new(NULL);
	gtk_container_add(GTK_CONTAINER(app->window), app->status);
	int sockets[2];
	g_assert_cmpint(socketpair(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0, sockets), ==, 0);
	ConnectionRequest *request = g_new0(ConnectionRequest, 1);
	g_mutex_init(&request->widgets_ready);
	request->type = WIRE_HELLO;
	GTask *task = g_task_new(NULL, NULL, NULL, NULL);
	g_task_set_task_data(task, request, connection_request_free);
	g_task_return_int(task, sockets[0]);
	app->refs++;
	connected(NULL, G_ASYNC_RESULT(task), app);
	g_object_unref(task);
	g_assert_true(gtk_widget_get_visible(app->status));
	g_assert_cmpstr(gtk_label_get_text(GTK_LABEL(app->status)), ==, "Connecting to waiting-test...");
	g_assert_false(app->attached);
	/* Accepting the catalog, rather than dispatching a partial response, owns readiness. */
	uint32_t empty[3] = {0};
	WirePacket packet = {.data = (unsigned char *)empty, .len = sizeof(empty)};
	g_assert_true(catalog(app, &packet));
	g_assert_true(app->attached);
	g_assert_false(gtk_widget_get_visible(app->status));
	app->closing = TRUE;
	disconnected(app);
	close(sockets[1]);
	gtk_widget_destroy(app->window);
	apps = g_list_remove(apps, app);
	app_unref(app);
}
