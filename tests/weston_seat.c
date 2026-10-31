/* Headless Weston needs a seat for real GTK IME/clipboard paths. The internal
 * backend entrypoints have matching signatures in supported Weston 10 and 16;
 * compile this module against the public structs for the selected runtime ABI. */
#include <stdlib.h>
#include <libweston/libweston.h>
#include "wayland_focus.h"

void weston_seat_init(struct weston_seat *, struct weston_compositor *, const char *);
int weston_seat_init_keyboard(struct weston_seat *, struct xkb_keymap *);
int weston_seat_init_pointer(struct weston_seat *);
void weston_seat_release(struct weston_seat *);

struct TestSeat {
	struct weston_seat seat;
	struct wl_listener destroy;
	struct wl_global *focus;
};

static void
activate(struct wl_client *client, struct wl_resource *resource, struct wl_resource *surface_resource)
{
	struct TestSeat *test = wl_resource_get_user_data(resource);
	if (wl_resource_get_client(surface_resource) != client) return;
	struct weston_surface *surface = wl_resource_get_user_data(surface_resource);
	struct weston_view *view;
	wl_list_for_each(view, &surface->views, surface_link) {
		if (!view->is_mapped) continue;
		weston_view_activate_input(view, &test->seat, WESTON_ACTIVATE_FLAG_CONFIGURE);
		return;
	}
}

static const struct { void (*activate)(struct wl_client *, struct wl_resource *, struct wl_resource *); }
	focus_implementation = { activate };

static void
bind_focus(struct wl_client *client, void *data, uint32_t version, uint32_t id)
{
	struct wl_resource *resource = wl_resource_create(client, &focus_interface, version, id);
	if (!resource) { wl_client_post_no_memory(client); return; }
	wl_resource_set_implementation(resource, &focus_implementation, data, NULL);
}

static void
release(struct wl_listener *listener, void *data)
{
	(void)data;
	struct TestSeat *test = wl_container_of(listener, test, destroy);
	wl_list_remove(&test->destroy.link);
	wl_global_destroy(test->focus);
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
	test->focus = wl_global_create(compositor->wl_display, &focus_interface, 1, test, bind_focus);
	if (!test->focus) {
		weston_seat_release(&test->seat);
		free(test);
		return -1;
	}
	test->destroy.notify = release;
	wl_signal_add(&compositor->destroy_signal, &test->destroy);
	return 0;
}
