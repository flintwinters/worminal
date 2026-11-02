/* Exercise the production loader with real modules, WebKit views, and live page state. */
#define _GNU_SOURCE
#include <webkit2/webkit2.h>
#include "../src/gtk/browser_loader.c"
#include "gtk_wait.h"
#include "webkit_script.c"

static int rejected;
static Browser *mirrored;
static const char *install_on_select;
void app_startuptime(const char *phase) { (void)phase; }
void app_status(App *app, const char *status)
{
	(void)app;
	if (status) rejected++;
}
void app_page_title(Page *page, const char *title) { g_free(page->title); page->title = g_strdup(title); }
void app_page_url(Page *page, const char *url) { g_free(page->url); page->url = g_strdup(url); }
void app_browser(App *app, const char *url) { (void)app; (void)url; }
void app_close(Page *page) { (void)page; }

Browser *app_browser_instance(Page *page) { return page->browser ? page->browser : mirrored; }

static void
publish(const char *source)
{
	char *path = module_path(WORMINAL_BROWSER_MODULE, NULL);
	char *pending = g_strconcat(path, ".pending", NULL), *bytes;
	gsize length;
	g_assert_true(g_file_get_contents(source, &bytes, &length, NULL));
	g_assert_true(g_file_set_contents(pending, bytes, length, NULL));
	g_assert_cmpint(g_rename(pending, path), ==, 0);
	g_free(bytes); g_free(path); g_free(pending);
}

static void
prepare_page(Page *page, App *app, const char *url)
{
	page->app = app;
	page->url = g_strdup(url);
	page->colors[259] = (GdkRGBA){0, 0, 0, 1};
	app->window = gtk_window_new(GTK_WINDOW_TOPLEVEL);
}

static void
create_page(Page *page)
{
	App *app = page->app;
	const char *url = page->url;
	page->browser = browser_new(app, url);
	g_assert_nonnull(page->browser);
	gtk_container_add(GTK_CONTAINER(app->window), browser_widget(page->browser));
	browser_bind(page->browser, page, url);
	gtk_widget_show_all(app->window);
}

static void
wait_page(Page *page)
{
	WAIT(!webkit_web_view_is_loading(WEBKIT_WEB_VIEW(browser_view(page->browser))) &&
	     page->title && !strcmp(page->title, "Fixture"));
}

static void
open_page(Page *page, App *app, const char *url)
{
	prepare_page(page, app, url); create_page(page); wait_page(page);
}

void app_select(Page *page)
{
	if (page->browser) return;
	if (mirrored) {
		page->browser = mirrored;
		browser_bind(mirrored, page, page->url);
	} else if (install_on_select) {
		publish(install_on_select); install_on_select = NULL;
		create_page(page);
	}
}

static void
close_page(Page *page)
{
	browser_free(page->browser);
	page->browser = NULL;
	gtk_widget_destroy(page->app->window);
	g_clear_pointer(&page->title, g_free); g_clear_pointer(&page->url, g_free);
}

static GtkWidget *
entry(GtkWidget *widget)
{
	if (GTK_IS_ENTRY(widget)) return widget;
	if (!GTK_IS_CONTAINER(widget)) return NULL;
	GList *children = gtk_container_get_children(GTK_CONTAINER(widget));
	GtkWidget *found = NULL;
	for (GList *item = children; item && !found; item = item->next) found = entry(item->data);
	g_list_free(children);
	return found;
}

static void
assert_version(Page *page, const char *placeholder)
{
	g_assert_true(browser_shortcut(page, GDK_KEY_k, GDK_CONTROL_MASK));
	GtkWidget *location = entry(browser_widget(page->browser));
	g_assert_nonnull(location);
	g_assert_cmpstr(gtk_entry_get_placeholder_text(GTK_ENTRY(location)), ==, placeholder);
	g_assert_true(browser_shortcut(page, GDK_KEY_Escape, 0));
}

static void
assert_context(Page *page)
{
	char *value = evaluate_view(WEBKIT_WEB_VIEW(browser_view(page->browser)),
	    "document.querySelector('input').value + ':' + window.savedState + ':' + sessionStorage.getItem('old-tab')");
	g_assert_cmpstr(value, ==, "unsaved form:42:private");
	g_free(value);
}

static void
assert_local_read(Page *page)
{
	WebKitWebView *view = WEBKIT_WEB_VIEW(browser_view(page->browser));
	char *value = evaluate_view(view,
	    "window.localRead='pending'; window.worminal.readFile(decodeURIComponent(location.pathname))"
	    ".then(bytes=>window.localRead=new TextDecoder().decode(bytes), ()=>window.localRead='denied'); 'asked'");
	g_free(value);
	WAIT(script_equals_view(view, "String(window.localRead.includes('local-generation'))", "true"));
}

int
main(int argc, char **argv)
{
	g_assert_cmpint(argc, ==, 8);
	gtk_init(&argc, &argv);
	App apps[6] = {{0}};
	Page pages[6] = {{0}};
	open_page(&pages[0], &apps[0], argv[4]);
	const BrowserModule *original = instance_api(pages[0].browser);
	char *value = evaluate_view(WEBKIT_WEB_VIEW(browser_view(pages[0].browser)),
	    "document.querySelector('input').value='unsaved form'; window.savedState=42;"
	    "sessionStorage.setItem('old-tab', 'private');"
	    "document.cookie='generation-session=shared; path=/'; localStorage.setItem('generation-shared', 'yes'); 'ok'");
	g_free(value);
	assert_version(&pages[0], "URL");
	open_page(&pages[4], &apps[4], argv[5]);
	assert_local_read(&pages[4]);
	value = evaluate_view(WEBKIT_WEB_VIEW(browser_view(pages[4].browser)),
	    "document.querySelector('input').value='old file form'; 'ok'");
	g_free(value);
	/* Identical installs must not accumulate new module copies or engine contexts. */
	publish(argv[1]);
	open_page(&pages[1], &apps[1], argv[4]);
	g_assert_true(instance_api(pages[1].browser) == original);
	close_page(&pages[1]);
	/* The module can be replaced during the host's claim/create callback. Shortcut
	 * dispatch must resolve the actual created instance, not an earlier module. */
	prepare_page(&pages[1], &apps[1], argv[4]);
	install_on_select = argv[2];
	g_assert_true(browser_shortcut(&pages[1], GDK_KEY_k, GDK_CONTROL_MASK));
	wait_page(&pages[1]);
	const BrowserModule *updated = instance_api(pages[1].browser);
	g_assert_true(updated != original);
	assert_version(&pages[1], "NEW");
	assert_version(&pages[0], "URL");
	assert_context(&pages[0]);
	value = evaluate_view(WEBKIT_WEB_VIEW(browser_view(pages[1].browser)),
	    "String(window.savedState) + ':' + sessionStorage.getItem('old-tab') + ':' + "
	    "document.cookie.includes('generation-session=shared') + ':' + localStorage.getItem('generation-shared')");
	g_assert_cmpstr(value, ==, "undefined:null:true:yes"); g_free(value);
	open_page(&pages[5], &apps[5], argv[6]);
	assert_local_read(&pages[5]); assert_local_read(&pages[4]);
	g_assert_true(script_equals_view(WEBKIT_WEB_VIEW(browser_view(pages[4].browser)),
	    "document.querySelector('input').value", "old file form"));
	/* A passive mirror dispatches through its peer's original module, too. */
	Page mirror = {.app = pages[0].app, .url = g_strdup(pages[0].url)};
	mirror.colors[259] = pages[0].colors[259];
	mirrored = pages[0].browser;
	g_assert_true(browser_shortcut(&mirror, GDK_KEY_k, GDK_CONTROL_MASK));
	mirrored = NULL;
	assert_version(&mirror, "URL");
	browser_bind(pages[0].browser, &pages[0], pages[0].url);
	g_free(mirror.title); g_free(mirror.url);
	assert_context(&pages[0]);
	publish(argv[3]);
	open_page(&pages[2], &apps[2], argv[4]);
	g_assert_cmpint(rejected, ==, 1);
	g_assert_true(instance_api(pages[2].browser) == updated);
	assert_version(&pages[2], "NEW");
	assert_context(&pages[0]);
	close_page(&pages[2]);
	publish(argv[7]);
	open_page(&pages[2], &apps[2], argv[4]);
	g_assert_cmpint(rejected, ==, 2);
	g_assert_true(instance_api(pages[2].browser) == updated);
	assert_context(&pages[0]);
	close_page(&pages[2]);
	/* Rolling back creates new old-version views without changing newer live views. */
	publish(argv[1]);
	open_page(&pages[3], &apps[3], argv[4]);
	g_assert_true(instance_api(pages[3].browser) == original);
	assert_version(&pages[3], "URL");
	assert_version(&pages[1], "NEW");
	assert_context(&pages[0]);
	close_page(&pages[3]); close_page(&pages[1]); close_page(&pages[0]);
	close_page(&pages[4]); close_page(&pages[5]);
	g_assert_cmpuint(g_hash_table_size(instances), ==, 0);
	g_print("Browser generations: new code, preserved page state, shared profile, rejection and rollback passed.\n");
	return 0;
}
