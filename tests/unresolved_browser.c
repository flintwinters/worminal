/* Valid module entry with a dependency that must fail before the factory can run. */
#include "gtk/browser_module.h"
extern void worminal_missing_browser_dependency(void);
static Browser *create(App *app, const char *url)
{
	(void)app; (void)url;
	worminal_missing_browser_dependency();
	return NULL;
}
static GtkWidget *widget(Browser *b) { (void)b; return NULL; }
static void bind(Browser *b, Page *p, const char *url) { (void)b; (void)p; (void)url; }
static gboolean key(Page *p, guint keyval, guint mods) { (void)p; (void)keyval; (void)mods; return FALSE; }
static void zoom(Browser *b, double value) { (void)b; (void)value; }
static void destroy(Browser *b) { (void)b; }
static void initialize(const BrowserHost *host) { (void)host; }
G_MODULE_EXPORT const BrowserModule *
worminal_browser_module(unsigned int abi)
{
	static const BrowserModule module = {create, widget, widget, bind, key, zoom, destroy, initialize};
	return abi == WORMINAL_BROWSER_ABI ? &module : NULL;
}
