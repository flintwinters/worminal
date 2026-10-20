/* Terminal launches must not load WebKit. Keep its callbacks resident once used. */
#include "browser_module.h"

static const BrowserModule *browser;

static gboolean
load_browser(App *app)
{
	if (browser) return TRUE;
	char *executable = g_file_read_link("/proc/self/exe", NULL);
	char *directory = executable ? g_path_get_dirname(executable) : NULL;
	g_free(executable);
	if (!directory) { app_status(app, "Cannot locate the browser module."); return FALSE; }
	char *path = g_build_filename(directory, "..", "lib", "worminal", "browser.so", NULL);
	if (!g_file_test(path, G_FILE_TEST_EXISTS)) {
		g_free(path);
		path = g_build_filename(directory, "build", "browser.so", NULL);
	}
	g_free(directory);
	GModule *library = g_module_open(path, G_MODULE_BIND_LAZY | G_MODULE_BIND_LOCAL);
	g_free(path);
	const BrowserModule *(*entry)(unsigned int) = NULL;
	if (library && g_module_symbol(library, "worminal_browser_module", (gpointer *)&entry))
		browser = entry(WORMINAL_BROWSER_ABI);
	if (!browser) {
		char *message = g_strdup_printf("Cannot load browser module: %s", g_module_error() ?: "incompatible ABI");
		app_status(app, message);
		g_free(message);
		if (library) g_module_close(library);
		return FALSE;
	}
	g_module_make_resident(library);
	return TRUE;
}

Browser *browser_new(App *app) { return load_browser(app) ? browser->create(app) : NULL; }
GtkWidget *browser_widget(Browser *b) { return browser->widget(b); }
GtkWidget *browser_view(Browser *b) { return browser->view(b); }
void browser_bind(Browser *b, Page *p, const char *url) { browser->bind(b, p, url); }
gboolean browser_shortcut(Page *p, guint key, guint mods)
{ return load_browser(p->app) && browser->shortcut(p, key, mods); }
void browser_zoom(Browser *b, double zoom) { browser->zoom(b, zoom); }
void browser_free(Browser *b) { browser->destroy(b); }
