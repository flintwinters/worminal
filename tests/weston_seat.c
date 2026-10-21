/* Headless Weston needs a seat for real GTK IME/clipboard paths. The internal
 * backend entrypoints have matching signatures in supported Weston 10 and 16;
 * compile this module against the public structs for the selected runtime ABI. */
#include <stdlib.h>
#include <libweston/libweston.h>

void weston_seat_init(struct weston_seat *, struct weston_compositor *, const char *);
int weston_seat_init_keyboard(struct weston_seat *, struct xkb_keymap *);
int weston_seat_init_pointer(struct weston_seat *);
void weston_seat_release(struct weston_seat *);

struct TestSeat {
	struct weston_seat seat;
	struct wl_listener destroy;
};

static void
release(struct wl_listener *listener, void *data)
{
	(void)data;
	struct TestSeat *test = wl_container_of(listener, test, destroy);
	wl_list_remove(&test->destroy.link);
	weston_seat_release(&test->seat);
	free(test);
}

WL_EXPORT int
wet_module_init(struct weston_compositor *compositor, int *argc, char *argv[])
{
	(void)argc; (void)argv;
	struct TestSeat *test = calloc(1, sizeof(*test));
	if (!test) return -1;
	weston_seat_init(&test->seat, compositor, "worminal-test-seat");
	if (weston_seat_init_keyboard(&test->seat, NULL) < 0 || weston_seat_init_pointer(&test->seat) < 0) {
		weston_seat_release(&test->seat);
		free(test);
		return -1;
	}
	test->destroy.notify = release;
	wl_signal_add(&compositor->destroy_signal, &test->destroy);
	return 0;
}
