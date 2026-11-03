/* Private headless-compositor protocol. Never built into product modules. */
#ifndef WORMINAL_TEST_FOCUS_H
#define WORMINAL_TEST_FOCUS_H
#include <wayland-util.h>

static const struct wl_interface *focus_types[] = { &wl_surface_interface };
static const struct wl_message focus_requests[] = {
	{ "activate", "o", focus_types },
};
static const struct wl_interface focus_interface = {
	"worminal_test_focus", 1, 1, focus_requests, 0, NULL,
};
#endif
