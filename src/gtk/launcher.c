/* Most additional launches only need GLib and the existing owner's socket. */
#include <string.h>
#include <unistd.h>
#include "modules.h"
#include "windows.h"

static void
timing(const char *phase)
{
	if (g_getenv("WORMINAL_TRACE_TIMING"))
		g_printerr("worminal-timing %s %" G_GINT64_FORMAT "\n", phase, g_get_monotonic_time() * 1000);
}

int
main(int argc, char **argv)
{
	timing("main");
	/* These commands must run locally even when a GTK owner already exists. */
	gboolean local = argc >= 2 && (!strcmp(argv[1], "--new-browser-tab") ||
	    (argc == 2 && (!strcmp(argv[1], "-h") || !strcmp(argv[1], "--help") ||
	                   !strcmp(argv[1], "-v") || !strcmp(argv[1], "--version"))));
	int listener = -1;
	if (!local) {
		int forwarded = windows_forward(argc, argv, &listener);
		timing("broker");
		if (forwarded) {
			if (forwarded < 0) g_printerr("Could not open a window in the local GTK frontend.\n");
			return forwarded > 0 ? 0 : 1;
		}
	}
	char *path = module_path("frontend.so");
	/* Explicit module callbacks keep GTK out of the global symbol search scope. */
	GModule *module = path ? g_module_open(path, G_MODULE_BIND_LAZY | G_MODULE_BIND_LOCAL) : NULL;
	g_free(path);
	int (*entry)(int, char **, int) = NULL;
	if (!module || !g_module_symbol(module, "worminal_frontend_v1", (gpointer *)&entry)) {
		g_printerr("Cannot load terminal frontend: %s\n", g_module_error() ?: "module path unavailable");
		if (listener >= 0) close(listener);
		if (module) g_module_close(module);
		return 1;
	}
	g_module_make_resident(module);
	return entry(argc, argv, listener);
}
