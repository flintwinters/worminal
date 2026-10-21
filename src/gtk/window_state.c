/* Read-only session-bus state; window selection belongs to GTK, not worminald. */
#include <string.h>
#include <unistd.h>
#include "preview.h"

#define STATE_PATH "/org/worminal/Windows"
#define STATE_INTERFACE "org.worminal.Windows1"
#define WINDOWS_TYPE "a(ubbays)"

static GList *tracked;
static guint next_id, owner;
static GDBusConnection *bus;
static GVariant *published;

static GVariant *
window_snapshot(void)
{
	GVariantBuilder builder;
	g_variant_builder_init(&builder, G_VARIANT_TYPE(WINDOWS_TYPE));
	for (GList *item = tracked; item; item = item->next) {
		App *app = item->data;
		Page *page = app->active;
		gboolean available = !app->closing && app->fd >= 0 && app->attached &&
		    page && page->terminal && page->directory && page->directory[0] == '/';
		const char *directory = available ? page->directory : "";
		GVariant *bytes = g_variant_new_fixed_array(G_VARIANT_TYPE_BYTE,
		    directory, strlen(directory), sizeof(guint8));
		g_variant_builder_add(&builder, "(ubb@ays)", app->state_id,
		    gtk_window_is_active(GTK_WINDOW(app->window)), available, bytes,
		    app->master ? app->master : "");
	}
	return g_variant_ref_sink(g_variant_builder_end(&builder));
}

void
window_state_changed(App *app)
{
	(void)app;
	if (!bus) return;
	GVariant *snapshot = window_snapshot();
	if (!published || !g_variant_equal(published, snapshot)) {
		g_dbus_connection_emit_signal(bus, NULL, STATE_PATH, STATE_INTERFACE,
		    "Changed", g_variant_new("(@" WINDOWS_TYPE ")", g_variant_ref(snapshot)), NULL);
		g_clear_pointer(&published, g_variant_unref);
		published = g_variant_ref(snapshot);
	}
	g_variant_unref(snapshot);
}

static void
get_windows(GDBusConnection *connection, const char *sender, const char *path,
    const char *interface, const char *method, GVariant *parameters,
    GDBusMethodInvocation *invocation, gpointer data)
{
	(void)connection; (void)sender; (void)path; (void)interface;
	(void)method; (void)parameters; (void)data;
	g_dbus_method_invocation_return_value(invocation,
	    g_variant_new("(@" WINDOWS_TYPE ")", window_snapshot()));
}

static void
bus_acquired(GDBusConnection *connection, const char *name, gpointer data)
{
	(void)name; (void)data;
	static const GDBusInterfaceVTable vtable = {.method_call = get_windows};
	const char *xml = "<node><interface name='" STATE_INTERFACE "'>"
	    "<method name='GetWindows'><arg direction='out' type='" WINDOWS_TYPE "'/></method>"
	    "<signal name='Changed'><arg type='" WINDOWS_TYPE "'/></signal>"
	    "</interface></node>";
	GDBusNodeInfo *info = g_dbus_node_info_new_for_xml(xml, NULL);
	if (g_dbus_connection_register_object(connection, STATE_PATH, info->interfaces[0],
	    &vtable, NULL, NULL, NULL)) {
		bus = connection;
		window_state_changed(NULL);
	}
	g_dbus_node_info_unref(info);
}

static void
bus_lost(GDBusConnection *connection, const char *name, gpointer data)
{
	(void)connection; (void)name; (void)data;
	bus = NULL;
	g_clear_pointer(&published, g_variant_unref);
}

void
window_state_add(App *app)
{
	app->state_id = ++next_id;
	tracked = g_list_append(tracked, app);
	g_signal_connect_swapped(app->window, "notify::is-active", G_CALLBACK(window_state_changed), app);
	if (!owner) {
		char *name = g_strdup_printf("org.worminal.Instance.p%ld", (long)getpid());
		owner = g_bus_own_name(G_BUS_TYPE_SESSION, name, G_BUS_NAME_OWNER_FLAGS_NONE,
		    bus_acquired, NULL, bus_lost, NULL, NULL);
		g_free(name);
	}
	window_state_changed(app);
}

void
window_state_remove(App *app)
{
	tracked = g_list_remove(tracked, app);
	window_state_changed(NULL);
}
