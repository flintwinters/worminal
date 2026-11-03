/* Wayland clients cannot force activation without a compositor-issued token.
 * Synthetic GTK events carry no such token. Ask only our private test compositor
 * to change real keyboard focus; assertions still observe GDK's actual state. */
#include <gdk/gdkwayland.h>
#include <wayland-client.h>
#include "wayland_focus.h"

static struct wl_proxy *test_focus;

static void
focus_global(void *data, struct wl_registry *registry, uint32_t name, const char *interface, uint32_t version)
{
	(void)data; (void)version;
	if (!strcmp(interface, focus_interface.name))
		test_focus = wl_registry_bind(registry, name, &focus_interface, 1);
}

static void
focus_removed(void *data, struct wl_registry *registry, uint32_t name)
{
	(void)data; (void)registry; (void)name;
}

static void
present_test_window(GtkWidget *window)
{
	gtk_window_present(GTK_WINDOW(window));
	GdkDisplay *display = gtk_widget_get_display(window);
	if (!GDK_IS_WAYLAND_DISPLAY(display)) return;
	struct wl_display *native = gdk_wayland_display_get_wl_display(display);
	if (!test_focus) {
		struct wl_registry *registry = wl_display_get_registry(native);
		static const struct wl_registry_listener listener = { focus_global, focus_removed };
		wl_registry_add_listener(registry, &listener, NULL);
		g_assert_cmpint(wl_display_roundtrip(native), >=, 0);
		wl_registry_destroy(registry);
		g_assert_nonnull(test_focus);
	}
	wl_proxy_marshal(test_focus, 0, gdk_wayland_window_get_wl_surface(gtk_widget_get_window(window)));
	g_assert_cmpint(wl_display_flush(native), >=, 0);
}
