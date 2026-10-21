/* Observe actual wire requests: passive GTK events must not claim the PTY. */
static void
check_passive_ownership(Terminal *t)
{
	Page *page = t->page;
	App *app = page->app;
	int sockets[2];
	g_assert_cmpint(socketpair(AF_UNIX, SOCK_STREAM, 0, sockets), ==, 0);
	int previous_fd = app->fd;
	uint32_t previous_mode = page->mode;
	gboolean previous_live = page->live;
	app->fd = sockets[0];
	page->live = FALSE;
	page->mode = MODE_MOUSEMANY | MODE_MOUSESGR | MODE_FOCUS;
	GdkEventFocus event = {.type = GDK_FOCUS_CHANGE, .in = TRUE};
	focus(page->widget, &event, t);
	GdkEventMotion pointer = {.type = GDK_MOTION_NOTIFY, .x = 40, .y = 40};
	motion(page->widget, &pointer, t);
	event.in = FALSE;
	focus(page->widget, &event, t);
	WireReader reader = {0};
	WirePacket packet;
	for (int i = 0; i < 2; i++) {
		g_assert_cmpint(wire_tryread(sockets[1], &reader, &packet), ==, 1);
		g_assert_cmpuint(packet.type, ==, WIRE_FOCUS_STATE);
		wire_packet_free(&packet);
	}
	g_assert_cmpint(wire_tryread(sockets[1], &reader, &packet), ==, 0);
	/* Explicit clicks still claim an inactive replica and report the button. */
	GdkEventButton click = {.type = GDK_BUTTON_PRESS, .button = 1, .x = 40, .y = 40};
	button(page->widget, &click, t);
	g_assert_cmpint(wire_tryread(sockets[1], &reader, &packet), ==, 1);
	g_assert_cmpuint(packet.type, ==, WIRE_FOCUS);
	wire_packet_free(&packet);
	/* Input can repeat the claim until the service acknowledges ownership. */
	while (wire_tryread(sockets[1], &reader, &packet) == 1) {
		g_assert_true(packet.type == WIRE_FOCUS || packet.type == WIRE_INPUT);
		wire_packet_free(&packet);
	}
	/* The live controller retains application-requested pointer reporting. */
	page->live = TRUE;
	pointer.x += 40;
	motion(page->widget, &pointer, t);
	g_assert_cmpint(wire_tryread(sockets[1], &reader, &packet), ==, 1);
	g_assert_cmpuint(packet.type, ==, WIRE_INPUT);
	wire_packet_free(&packet);
	app->fd = previous_fd;
	page->mode = previous_mode;
	page->live = previous_live;
	t->button = 0; t->input_pending = FALSE;
	wire_reader_free(&reader);
	close(sockets[0]); close(sockets[1]);
}
