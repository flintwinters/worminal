/* Each live browser keeps the module that created it; new tabs inspect the installed module. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>
#include <glib/gstdio.h>
#include "browser_module.h"
#include "modules.h"

typedef struct Generation Generation;
struct Generation {
	const BrowserModule *api;
	void (*retired)(gboolean);
	char *digest;
	struct stat source;
	int snapshot;
	Generation *next;
};
static Generation *generations, *latest;
static GHashTable *instances;
static const BrowserHost callbacks = {
	app_page_title, app_page_url, app_startuptime, app_browser, app_close, app_select
};

static gboolean
same_source(const struct stat *a, const struct stat *b)
{
	return a->st_dev == b->st_dev && a->st_ino == b->st_ino && a->st_size == b->st_size &&
	    a->st_mtim.tv_sec == b->st_mtim.tv_sec && a->st_mtim.tv_nsec == b->st_mtim.tv_nsec;
}

static gboolean
read_module(const char *path, struct stat *source, char **bytes, gsize *length)
{
	int fd = g_open(path, O_RDONLY | O_CLOEXEC | O_NONBLOCK, 0);
	struct stat after;
	if (fd < 0) return FALSE;
	if (fstat(fd, source) < 0 || !S_ISREG(source->st_mode) || source->st_size < 0 ||
	    (uintmax_t)source->st_size > G_MAXSIZE) { close(fd); return FALSE; }
	*length = source->st_size;
	*bytes = g_malloc(*length);
	gsize received = 0;
	while (received < *length) {
		ssize_t count = read(fd, *bytes + received, *length - received);
		if (count < 0 && errno == EINTR) continue;
		if (count <= 0) break;
		received += count;
	}
	/* Bind bytes to the opened inode, not a separate pathname read. An install
	 * racing this copy must not assign a different module's identity to these bytes. */
	gboolean stable = received == *length && fstat(fd, &after) == 0 && same_source(source, &after) &&
	    g_stat(path, &after) == 0 && same_source(source, &after);
	close(fd);
	if (!stable) { g_clear_pointer(bytes, g_free); return FALSE; }
	return TRUE;
}

static int
snapshot(const char *bytes, gsize length, const char *digest)
{
	int fd = memfd_create(digest, MFD_CLOEXEC | MFD_ALLOW_SEALING);
	if (fd < 0) return -1;
	for (gsize sent = 0; sent < length;) {
		ssize_t count = write(fd, bytes + sent, length - sent);
		if (count < 0 && errno == EINTR) continue;
		if (count <= 0) { close(fd); return -1; }
		sent += count;
	}
	if (fcntl(fd, F_ADD_SEALS, F_SEAL_WRITE | F_SEAL_GROW | F_SEAL_SHRINK | F_SEAL_SEAL) < 0) {
		close(fd); return -1;
	}
	return fd;
}

static Generation *
load_generation(App *app)
{
	char *path = module_path(WORMINAL_BROWSER_MODULE, "browser.so");
	struct stat source;
	if (!path || g_stat(path, &source) < 0 || !S_ISREG(source.st_mode)) goto failed;
	if (latest && same_source(&source, &latest->source)) { g_free(path); return latest; }
	app_startuptime("browser-module-start");
	char *bytes = NULL;
	gsize length;
	if (!read_module(path, &source, &bytes, &length)) goto failed;
	char *digest = g_compute_checksum_for_data(G_CHECKSUM_SHA256, (const guchar *)bytes, length);
	Generation *generation;
	for (generation = generations; generation; generation = generation->next)
		if (!strcmp(generation->digest, digest)) break;
	if (!generation) {
		int fd = snapshot(bytes, length, digest);
		g_free(bytes); bytes = NULL;
		/* Debuggers must open this process's fd, not their own /proc/self/fd. */
		char *name = g_strdup_printf("/proc/%ld/fd/%d", (long)getpid(), fd);
		/* Resolve dependencies before activation: missing symbols must reject
		 * an update here, not abort the owner later while old views are live. */
		GModule *library = fd >= 0 ? g_module_open(name, G_MODULE_BIND_LOCAL) : NULL;
		g_free(name);
		const BrowserModule *(*entry)(unsigned int) = NULL;
		const BrowserModule *api = library &&
		    g_module_symbol(library, "worminal_browser_module", (gpointer *)&entry)
		    ? entry(WORMINAL_BROWSER_ABI) : NULL;
		if (!api) {
			if (library) g_module_close(library);
			if (fd >= 0) close(fd);
			g_free(digest); goto failed;
		}
		generation = g_new0(Generation, 1);
		generation->api = api;
		generation->snapshot = fd;
		generation->digest = digest;
		g_module_symbol(library, "worminal_browser_set_retired_v1", (gpointer *)&generation->retired);
		api->initialize(&callbacks);
		/* WebKit may retain asynchronous callbacks after views close. Code stays
		 * resident; retired modules release their unused engine context separately.
		 * Keep the sealed fd open so GModule's pathname cache cannot reuse its name. */
		g_module_make_resident(library);
		generation->next = generations; generations = generation;
	} else {
		g_free(bytes); g_free(digest);
	}
	generation->source = source;
	if (generation != latest) {
		if (latest && latest->retired) latest->retired(TRUE);
		if (generation->retired) generation->retired(FALSE);
		latest = generation;
	}
	g_free(path);
	app_startuptime("browser-module-ready");
	return generation;
failed:
	g_free(path);
	app_status(app, latest ? "Browser update rejected; using the previous browser code." :
	    "Cannot load a compatible browser module.");
	return latest;
}

static const BrowserModule *
instance_api(Browser *browser)
{
	Generation *generation = instances ? g_hash_table_lookup(instances, browser) : NULL;
	g_assert_nonnull(generation);
	return generation->api;
}

Browser *
browser_new(App *app, const char *url)
{
	Generation *generation = load_generation(app);
	if (!generation) return NULL;
	Browser *browser = generation->api->create(app, url);
	if (browser) {
		if (!instances) instances = g_hash_table_new(g_direct_hash, g_direct_equal);
		g_hash_table_insert(instances, browser, generation);
	}
	return browser;
}
GtkWidget *browser_widget(Browser *b) { return instance_api(b)->widget(b); }
GtkWidget *browser_view(Browser *b) { return instance_api(b)->view(b); }
void browser_bind(Browser *b, Page *p, const char *url) { instance_api(b)->bind(b, p, url); }
gboolean browser_shortcut(Page *p, guint key, guint mods)
{
	Browser *browser = app_browser_instance(p);
	/* Claim first: publication during creation may choose another generation.
	 * Only the actual instance's module may interpret its private Browser state. */
	if (!browser) { app_select(p); browser = app_browser_instance(p); }
	return browser && instance_api(browser)->shortcut(p, key, mods);
}
void browser_zoom(Browser *b, double zoom) { instance_api(b)->zoom(b, zoom); }
void browser_free(Browser *b)
{
	instance_api(b)->destroy(b);
	g_hash_table_remove(instances, b);
}
