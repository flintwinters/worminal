#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <locale.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <unistd.h>
#include <glib-unix.h>
#include "preview.h"
#include "workspace_client.h"
#include "workspace_socket.h"
#include "tab_label.h"
#include "windows.h"

static int windows;
static GList *apps;
enum { OUTPUT_LIMIT = 1024 * 1024 };
static void connect_async(App *);
static void disconnected(App *);
static gboolean shortcut(GtkWidget *, GdkEventKey *, App *);

void
app_startuptime(const char *event)
{
	if (g_getenv("WORMINAL_TRACE_TIMING"))
		g_printerr("worminal-timing %s %" G_GINT64_FORMAT "\n", event, g_get_monotonic_time() * 1000);
}

static void
app_unref(App *app)
{
	if (--app->refs) return;
	g_free(app->master); g_free(app->title); g_free(app->cwd); g_free(app->line);
	g_free(app->output); g_free(app->font); g_free(app->initial_url);
	g_clear_pointer(&app->cached_font, pango_font_description_free);
	g_strfreev(app->command); g_strfreev(app->environment);
	if (app->print_fd != STDOUT_FILENO) close(app->print_fd);
	g_free(app);
}

void
app_status(App *app, const char *message)
{
	gtk_label_set_text(GTK_LABEL(app->status), message ? message : "");
	gtk_widget_set_visible(app->status, message != NULL);
}

static gboolean
flush(int fd, GIOCondition condition, gpointer data)
{
	App *app = data;
	if (condition & (G_IO_HUP | G_IO_ERR)) {
		app->writer = 0;
		disconnected(app);
		return G_SOURCE_REMOVE;
	}
	while (app->sent < app->output_queue.len) {
		ssize_t count = send(fd, app->output_queue.data + app->sent,
		                     app->output_queue.len - app->sent, MSG_DONTWAIT | MSG_NOSIGNAL);
		if (count < 0 && errno == EINTR) continue;
		if (count < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) return G_SOURCE_CONTINUE;
		if (count <= 0) {
			app->writer = 0;
			disconnected(app);
			return G_SOURCE_REMOVE;
		}
		app->sent += count;
	}
	app->output_queue.len = app->sent = 0;
	app->writer = 0;
	return G_SOURCE_REMOVE;
}

void
app_send(App *app, uint32_t type, uint32_t tab, const void *bytes, size_t length)
{
	if (app->fd < 0 || app->closing) return;
	if (app->sent) {
		memmove(app->output_queue.data, app->output_queue.data + app->sent, app->output_queue.len - app->sent);
		app->output_queue.len -= app->sent;
		app->sent = 0;
	}
	if (length + 16 > OUTPUT_LIMIT - app->output_queue.len) {
		disconnected(app);
		app_status(app, "Input queue full. Some input was not delivered; reconnecting.");
		return;
	}
	wire_append_packet(&app->output_queue, type, tab, bytes, length);
	if (!app->writer && flush(app->fd, 0, app) == G_SOURCE_CONTINUE)
		app->writer = g_unix_fd_add(app->fd, G_IO_OUT | G_IO_HUP | G_IO_ERR, flush, app);
}

static void
size_message(Page *page, uint32_t type)
{
	if (!page || !page->terminal || page->app->syncing) return;
	int cols, rows;
	terminal_dimensions(page->terminal, &cols, &rows);
	if (cols < 1 || rows < 1) return;
	WireBuffer b = {0};
	wire_put_u32(&b, cols); wire_put_u32(&b, rows);
	app_send(page->app, type, page->id, b.data, b.len);
	wire_buffer_free(&b);
}

void app_focus(Page *page) { size_message(page, WIRE_FOCUS); }
void app_resize(Page *page) { size_message(page, WIRE_RESIZE); }

static void
page_title(Page *page, const char *title)
{
	g_free(page->title);
	page->title = g_strdup(title ? title : "Worminal");
	int length;
	const char *name = page->terminal ? tab_label(page->directory ? page->directory : "?", &length) : page->title;
	char *label = page->terminal ? g_strndup(name, length) : g_strdup(name);
	gtk_label_set_text(GTK_LABEL(page->label), label);
	g_free(label);
	if (page->app->active == page)
		gtk_window_set_title(GTK_WINDOW(page->app->window), page->title);
	tabs_refresh(page->app);
}

void
app_page_title(Page *page, const char *title)
{
	/* One local WebKit page supplies the title for every mirror of its identity. */
	char *value = g_strdup(title ? title : "Worminal");
	page_title(page, value);
	if (page->url)
		for (GList *item = apps; item; item = item->next) {
			App *app = item->data;
			if (app == page->app || app->closing || g_strcmp0(app->master, page->app->master)) continue;
			for (GList *entry = app->pages; entry; entry = entry->next) {
				Page *peer = entry->data;
				if (peer->id == page->id && peer->url) page_title(peer, value);
			}
		}
	g_free(value);
}

static Page *
browser_peer(Page *page, gboolean live_only)
{
	for (GList *item = apps; item; item = item->next) {
		App *app = item->data;
		if (app == page->app || app->closing || g_strcmp0(app->master, page->app->master)) continue;
		for (GList *entry = app->pages; entry; entry = entry->next) {
			Page *peer = entry->data;
			if (peer->id == page->id && peer->url && (!live_only || peer->browser)) return peer;
		}
	}
	return NULL;
}

static void
move_browser(Page *from, Page *to)
{
	to->browser = from ? from->browser : browser_new(to->app);
	if (!to->browser) return;
	if (from) {
		from->browser = NULL;
		gtk_container_remove(GTK_CONTAINER(from->widget), browser_widget(to->browser));
		gtk_widget_show(from->placeholder);
	}
	gtk_box_pack_start(GTK_BOX(to->widget), browser_widget(to->browser), TRUE, TRUE, 0);
	browser_bind(to->browser, to, to->url);
	gtk_widget_show_all(to->widget);
	gtk_widget_hide(to->placeholder);
}

static void
claim_browser(Page *page)
{
	if (!page->browser) move_browser(browser_peer(page, TRUE), page);
}

static void
activate(Page *page)
{
	App *app = page->app;
	app->active = page;
	if (page->url) {
		app->geometry_pending = FALSE;
		claim_browser(page);
		if (page->browser) gtk_widget_grab_focus(browser_view(page->browser));
	} else {
		app_focus(page);
		gtk_widget_grab_focus(page->widget);
	}
	gtk_window_set_title(GTK_WINDOW(app->window), page->title ? page->title : "Worminal");
	tabs_refresh(app);
	/* Map early without dropping keys received before the first service catalog. */
	while (!g_queue_is_empty(&app->early_keys)) {
		GdkEvent *event = g_queue_pop_head(&app->early_keys);
		if (!shortcut(app->window, &event->key, app) && app->active->terminal)
			terminal_key(app->active->terminal, &event->key);
		gdk_event_free(event);
	}
}

void
app_select(Page *page)
{
	GtkNotebook *notebook = GTK_NOTEBOOK(page->app->notebook);
	int index = gtk_notebook_page_num(notebook, page->widget);
	gboolean same = index == gtk_notebook_get_current_page(notebook);
	gtk_notebook_set_current_page(notebook, index);
	/* Selecting an already selected mirror must still move its live browser. */
	if (same) activate(page);
}

static void
select_index(App *app, int index)
{
	GtkWidget *widget = gtk_notebook_get_nth_page(GTK_NOTEBOOK(app->notebook), index);
	for (GList *item = app->pages; item; item = item->next) {
		Page *page = item->data;
		if (page->widget == widget) { app_select(page); return; }
	}
}

static void
switched(GtkNotebook *notebook, GtkWidget *widget, guint index, App *app)
{
	(void)notebook; (void)index;
	if (app->syncing || app->closing) return;
	for (GList *item = app->pages; item; item = item->next) {
		Page *page = item->data;
		if (page->widget == widget) { activate(page); break; }
	}
}

void app_close(Page *page) { app_send(page->app, WIRE_CLOSE, page->id, NULL, 0); }

void
page_free(Page *page)
{
	if (page->terminal) terminal_free(page->terminal);
	if (page->placeholder) g_signal_handlers_disconnect_by_data(page->placeholder, page);
	if (page->browser) {
		Page *peer = browser_peer(page, FALSE);
		if (peer) move_browser(page, peer);
		if (page->browser) browser_free(page->browser);
	}
	if (!page->app->closing) gtk_widget_destroy(page->widget);
	g_object_unref(page->widget);
	g_free(page->url); g_free(page->directory); g_free(page->title);
	g_free(page);
}

static Page *
find_page(App *app, uint32_t id)
{
	for (GList *item = app->pages; item; item = item->next) {
		Page *p = item->data;
		if (p->id == id) return p;
	}
	return NULL;
}

static Page *
new_page(App *app, uint32_t id, gboolean browser)
{
	Page *page = g_new0(Page, 1);
	page->app = app;
	page->id = id;
	page->cursor = 2;
	for (size_t i = 0; i < G_N_ELEMENTS(page->colors); i++) terminal_palette(page, i, NULL);
	page->label = gtk_label_new(browser ? "Browser" : "Worminal");
	if (browser) {
		page->widget = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
		page->placeholder = gtk_button_new_with_label("Show browser here");
		gtk_widget_set_halign(page->placeholder, GTK_ALIGN_CENTER);
		gtk_widget_set_valign(page->placeholder, GTK_ALIGN_CENTER);
		g_signal_connect_swapped(page->placeholder, "clicked", G_CALLBACK(app_select), page);
		gtk_box_pack_start(GTK_BOX(page->widget), page->placeholder, TRUE, TRUE, 0);
	}
	else page->terminal = terminal_new(page);
	g_object_ref_sink(page->widget);
	app->pages = g_list_append(app->pages, page);
	gtk_notebook_append_page(GTK_NOTEBOOK(app->notebook), page->widget, page->label);
	gtk_widget_show(page->label);
	gtk_widget_show_all(page->widget);
	return page;
}

static gboolean
catalog(App *app, WirePacket *packet)
{
	uint32_t count, closed, fallback;
	if (!wire_get_u32(packet, &count) || !wire_get_u32(packet, &closed) ||
	    !wire_get_u32(packet, &fallback) || count > 4096) return FALSE;
	(void)closed;
	uint32_t selected_id = app->active ? app->active->id : 0, passive_conversion = 0;
	app->syncing = TRUE;
	for (GList *item = app->pages; item; item = item->next) ((Page *)item->data)->seen = FALSE;
	for (uint32_t index = 0; index < count; index++) {
		uint32_t id, kind, mode, cursor, colors;
		if (!wire_get_u32(packet, &id) || !wire_get_u32(packet, &kind) || kind > 1) goto invalid;
		char *url = wire_get_string(packet);
		if (!url) goto invalid;
		if (!wire_get_u32(packet, &mode) || !wire_get_u32(packet, &cursor)) { free(url); goto invalid; }
		char *title = wire_get_string(packet), *directory = wire_get_string(packet);
		if (!title || !directory || !wire_get_u32(packet, &colors) || colors > 270) {
			free(url); free(title); free(directory); goto invalid;
		}
		Page *page = find_page(app, id);
		if (page && !!page->terminal != !kind) {
			/* Only the live terminal owner automatically acquires the converted
			 * browser. A passive mirror must not steal it during catalog delivery. */
			if (!page->live) passive_conversion = id;
			if (app->active == page) app->active = NULL;
			app->pages = g_list_remove(app->pages, page);
			page_free(page);
			page = NULL;
		}
		if (!page) page = new_page(app, id, kind);
		page->seen = TRUE;
		g_free(page->directory); page->directory = directory;
		g_free(page->url); page->url = kind ? url : NULL;
		if (!kind) free(url);
		if (!page->browser) {
			Page *owner = kind ? browser_peer(page, TRUE) : NULL;
			app_page_title(page, owner && owner->title ? owner->title : title);
		}
		free(title);
		gboolean palette_changed = FALSE;
		for (uint32_t c = 0; c < colors; c++) {
			char *name = wire_get_string(packet);
			if (!name) goto invalid;
			palette_changed |= terminal_palette(page, c, name);
			free(name);
		}
		if (page->terminal) terminal_configure(page->terminal, mode, cursor, palette_changed);
		else { page->mode = mode; page->cursor = cursor; }
		/* Ordered identities come from the service; local notebook order follows it. */
		gtk_notebook_reorder_child(GTK_NOTEBOOK(app->notebook), page->widget, index);
	}
	if (packet->pos != packet->len) goto invalid;
	for (GList *item = app->pages, *next; item; item = next) {
		next = item->next;
		Page *page = item->data;
		if (page->seen) continue;
		if (app->active == page) app->active = NULL;
		app->pages = g_list_delete_link(app->pages, item);
		page_free(page);
	}
	app->syncing = FALSE;
	Page *chosen = find_page(app, app->created_id);
	app->created_id = 0;
	if (!chosen) chosen = find_page(app, selected_id);
	if (!chosen) chosen = app->active ? app->active : find_page(app, fallback);
	if (!chosen && app->pages) chosen = app->pages->data;
	if (chosen) {
		int index = gtk_notebook_page_num(GTK_NOTEBOOK(app->notebook), chosen->widget);
		gtk_notebook_set_current_page(GTK_NOTEBOOK(app->notebook), index);
		/* A peer catalog update must preserve local focus, forms and navigation. */
		if (chosen->id == passive_conversion) {
			app->active = chosen;
			gtk_window_set_title(GTK_WINDOW(app->window), chosen->title);
			tabs_refresh(app);
		} else if (chosen != app->active || !app->attached) activate(chosen);
	} else if (app->attached && !app->pending) gtk_widget_destroy(app->window);
	app->attached = TRUE;
	return TRUE;
invalid:
	app->syncing = FALSE;
	return FALSE;
}

static gboolean
received_packets(int fd, GIOCondition condition, gpointer data)
{
	App *app = data;
	/* Bound work per dispatch. wire_tryread never waits for a partial packet. */
	for (int i = 0; i < 64; i++) {
		WirePacket packet;
		int result = wire_tryread(fd, &app->input, &packet);
		if (result < 0) goto lost;
		if (!result) {
			if (condition & (G_IO_HUP | G_IO_ERR)) goto lost;
			return G_SOURCE_CONTINUE;
		}
		Page *page = find_page(app, packet.tab);
		gboolean okay = TRUE;
		if (packet.type == WIRE_CATALOG) okay = catalog(app, &packet);
		else if (packet.type == WIRE_CREATED) {
			app->created_id = packet.tab;
			if (app->pending) app->pending--;
		} else if (packet.type == WIRE_ERROR) {
			char *message = wire_get_string(&packet);
			app_status(app, message ? message : "Workspace error"); free(message);
		} else if (packet.type == WIRE_PRINT) {
			/* -o - retains the established CLI print stream. */
			if (packet.len) {
				ssize_t ignored = write(app->print_fd, packet.data, packet.len);
				(void)ignored;
			}
		} else if (page && packet.type == WIRE_RELEASE) {
			page->live = FALSE;
			gtk_widget_queue_draw(page->widget);
		} else if (page && packet.type == WIRE_BELL) {
			gtk_widget_error_bell(page->widget);
		} else if (page && packet.type == WIRE_CLIPBOARD) {
			char *value = wire_get_string(&packet);
			if (value && page->live) gtk_clipboard_set_text(gtk_clipboard_get(GDK_SELECTION_CLIPBOARD), value, -1);
			free(value);
		} else if (page && page->terminal) terminal_packet(page->terminal, &packet);
		wire_packet_free(&packet);
		if (app->closing) return G_SOURCE_REMOVE;
		if (!okay) goto lost;
	}
	return G_SOURCE_CONTINUE;
lost:
	app->reader = 0;
	disconnected(app);
	return G_SOURCE_REMOVE;
}

static gboolean
received(int fd, GIOCondition condition, gpointer data)
{
	App *app = data;
	app->refs++;
	gboolean result = received_packets(fd, condition, app);
	app_unref(app);
	return result;
}

static WireBuffer
terminal_request(App *app, gboolean initial)
{
	WireBuffer b = {0};
	wire_put_u32(&b, app->cols); wire_put_u32(&b, app->rows); wire_put_u32(&b, app->allowalt);
	uint32_t argc = initial && app->command ? g_strv_length(app->command) : 0;
	uint32_t envc = !app->master ? g_strv_length(app->environment) : 0;
	wire_put_u32(&b, argc);
	wire_put_u32(&b, envc);
	wire_put_string(&b, initial ? app->title : "Worminal");
	wire_put_string(&b, app->active && app->active->directory && *app->active->directory ?
	                      app->active->directory : app->master ? "" : app->cwd);
	wire_put_string(&b, initial ? app->line : NULL);
	wire_put_string(&b, initial ? app->output : NULL); wire_put_string(&b, "");
	for (uint32_t i = 0; i < argc; i++) wire_put_string(&b, app->command[i]);
	for (uint32_t i = 0; i < envc; i++) wire_put_string(&b, app->environment[i]);
	return b;
}

static WireBuffer
browser_request(const char *url)
{
	WireBuffer b = {0};
	wire_put_string(&b, url && *url ? url : "about:blank");
	return b;
}

void
app_terminal(App *app, gboolean initial)
{
	if (app->fd < 0) return;
	WireBuffer b = terminal_request(app, initial);
	app->pending++;
	app_send(app, initial ? WIRE_HELLO : WIRE_NEW, 0, b.data, b.len);
	wire_buffer_free(&b);
}

void
app_browser(App *app, const char *url)
{
	if (app->fd < 0) return;
	WireBuffer b = browser_request(url);
	app->pending++;
	app_send(app, WIRE_BROWSER, 0, b.data, b.len);
	wire_buffer_free(&b);
}

static gboolean
shortcut(GtkWidget *window, GdkEventKey *event, App *app)
{
	(void)window;
	if (!app->active) {
		if (g_queue_get_length(&app->early_keys) < 256)
			g_queue_push_tail(&app->early_keys, gdk_event_copy((GdkEvent *)event));
		return TRUE;
	}
	guint key = gdk_keyval_to_lower(event->keyval);
	guint mods = event->state & (GDK_CONTROL_MASK | GDK_SHIFT_MASK | GDK_MOD1_MASK);
	Page *page = app->active;
	if (page->url && browser_shortcut(page, key, mods)) return TRUE;
	if (mods == GDK_CONTROL_MASK && (key == GDK_KEY_t || key == GDK_KEY_n || key == GDK_KEY_w)) {
		if (key == GDK_KEY_t) app_terminal(app, FALSE);
		else if (key == GDK_KEY_n) app_new_window(app);
		else app_close(page);
		return TRUE;
	}
	if ((mods == GDK_CONTROL_MASK || mods == (GDK_CONTROL_MASK | GDK_SHIFT_MASK)) &&
	           (key == GDK_KEY_Tab || key == GDK_KEY_ISO_Left_Tab)) {
		int count = gtk_notebook_get_n_pages(GTK_NOTEBOOK(app->notebook));
		int index = gtk_notebook_get_current_page(GTK_NOTEBOOK(app->notebook));
		if (count) select_index(app, (index + (event->state & GDK_SHIFT_MASK ? count - 1 : 1)) % count);
		return TRUE;
	}
	if (mods == (GDK_CONTROL_MASK | GDK_SHIFT_MASK)) {
		if (key == GDK_KEY_t) app_terminal(app, FALSE);
		else if (key == GDK_KEY_n) app_new_window(app);
		else if (key == GDK_KEY_w && page) app_close(page);
		else if (key == GDK_KEY_c && page && page->terminal) terminal_copy(page->terminal, GDK_SELECTION_CLIPBOARD);
		else if (key == GDK_KEY_v && page && page->terminal) terminal_paste(page->terminal, GDK_SELECTION_CLIPBOARD);
		else if (key == GDK_KEY_y && page && page->terminal) terminal_paste(page->terminal, GDK_SELECTION_PRIMARY);
		else if (key == GDK_KEY_Home) app->zoom = 0;
		else if (key == GDK_KEY_Page_Up || key == GDK_KEY_plus) app->zoom += 1;
		else if (key == GDK_KEY_Page_Down) app->zoom -= 1;
		else return FALSE;
	} else if (mods == GDK_CONTROL_MASK && (key == GDK_KEY_equal || key == GDK_KEY_minus)) {
		app->zoom += key == GDK_KEY_equal ? 1 : -1;
	} else if ((mods == GDK_MOD1_MASK || mods == GDK_CONTROL_MASK) && key >= GDK_KEY_0 && key <= GDK_KEY_9) {
		int count = gtk_notebook_get_n_pages(GTK_NOTEBOOK(app->notebook));
		int index = key == GDK_KEY_9 ? count - 1 : key == GDK_KEY_0 ? 9 : (int)(key - GDK_KEY_1);
		select_index(app, MIN(index, count - 1));
		return TRUE;
	} else return FALSE;
	app->zoom = CLAMP(app->zoom, -10, 40);
	for (GList *item = app->pages; item; item = item->next) {
		Page *p = item->data;
		if (p->terminal) terminal_zoom(p->terminal);
		if (p->browser) browser_zoom(p->browser, app->zoom);
	}
	tabs_refresh(app);
	return TRUE;
}

static gboolean
retry(gpointer data)
{
	App *app = data;
	app->reconnect = 0;
	connect_async(app);
	return G_SOURCE_REMOVE;
}

static void
disconnected(App *app)
{
	if (app->reader) g_source_remove(app->reader);
	if (app->writer) g_source_remove(app->writer);
	app->reader = app->writer = 0;
	if (app->fd >= 0) close(app->fd);
	app->fd = -1;
	wire_reader_free(&app->input); app->input = (WireReader){0};
	wire_buffer_free(&app->output_queue); app->output_queue = (WireBuffer){0}; app->sent = 0;
	app->pending = 0;
	app->created_id = 0;
	for (GList *item = app->pages; item; item = item->next) ((Page *)item->data)->live = FALSE;
	if (!app->closing) app_status(app, "Workspace disconnected. Reconnecting; unsent input will not be replayed.");
	if (!app->closing && !app->reconnect) app->reconnect = g_timeout_add(1000, retry, app);
}

typedef struct {
	App *app;
	WireBuffer payload;
	uint32_t type;
} ConnectionRequest;

static void
connection_request_free(gpointer data)
{
	ConnectionRequest *request = data;
	wire_buffer_free(&request->payload);
	g_free(request);
}

static void
connect_worker(GTask *task, gpointer source, gpointer data, GCancellable *cancel)
{
	(void)source; (void)cancel;
	ConnectionRequest *request = data;
	App *app = request->app;
	int fd = workspace_client_open_process(app->master, &app->connecting_child);
	if (fd >= 0) {
		/* Launch the child while GTK maps the window. Only immutable wire data
		 * crosses this worker; all widgets and response parsing stay on GTK. */
		struct timeval timeout = {.tv_sec = 3};
		if (request->payload.len + 16 > OUTPUT_LIMIT ||
		    setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &timeout, sizeof(timeout)) < 0 ||
		    !wire_write(fd, request->type, 0, request->payload.data, request->payload.len)) {
			close(fd);
			fd = -1;
		} else app_startuptime("launch-sent");
	}
	g_task_return_int(task, fd);
}

static void
bridge_exited(GPid child, int status, gpointer data)
{
	(void)status; (void)data;
	g_spawn_close_pid(child);
}

static void
connected(GObject *source, GAsyncResult *result, gpointer data)
{
	(void)source;
	App *app = data;
	ConnectionRequest *request = g_task_get_task_data(G_TASK(result));
	int fd = g_task_propagate_int(G_TASK(result), NULL);
	app_startuptime("workspace");
	if (app->connecting_child) g_child_watch_add(app->connecting_child, bridge_exited, NULL);
	if (app->closing) { if (fd >= 0) close(fd); app_unref(app); return; }
	if (fd < 0 || fcntl(fd, F_SETFL, fcntl(fd, F_GETFL) | O_NONBLOCK) < 0) {
		if (fd >= 0) close(fd);
		disconnected(app);
	} else {
		app->fd = fd;
		app_status(app, NULL);
		/* Output floods must yield to GTK input and frame-clock redraw sources. */
		app->reader = g_unix_fd_add_full(G_PRIORITY_DEFAULT_IDLE, fd,
		    G_IO_IN | G_IO_HUP | G_IO_ERR, received, app, NULL);
		if (request->type == WIRE_ATTACH) app->attached = FALSE;
	}
	app_unref(app);
}

static void
connect_async(App *app)
{
	app->refs++;
	ConnectionRequest *request = g_new0(ConnectionRequest, 1);
	request->app = app;
	if (app->attached || app->initial_attach) request->type = WIRE_ATTACH;
	else {
		app->pending++;
		request->type = app->initial_url ? WIRE_BROWSER : WIRE_HELLO;
		request->payload = app->initial_url ? browser_request(app->initial_url) : terminal_request(app, TRUE);
	}
	GTask *task = g_task_new(NULL, NULL, connected, app);
	g_task_set_task_data(task, request, connection_request_free);
	g_task_run_in_thread(task, connect_worker);
	g_object_unref(task);
}

static void
destroyed(GtkWidget *widget, App *app)
{
	(void)widget;
	app->closing = TRUE;
	g_signal_handlers_disconnect_by_data(app->notebook, app);
	g_signal_handlers_disconnect_by_data(app->window, app);
	if (app->reconnect) g_source_remove(app->reconnect);
	app->reconnect = 0;
	disconnected(app);
	while (app->pages) {
		Page *page = app->pages->data;
		app->pages = g_list_delete_link(app->pages, app->pages);
		page_free(page);
	}
	g_queue_clear_full(&app->early_keys, (GDestroyNotify)gdk_event_free);
	apps = g_list_remove(apps, app);
	app_unref(app);
	if (!--windows && gtk_main_level()) gtk_main_quit();
}

static App *
new_app(void)
{
	App *app = g_new0(App, 1);
	app->refs = 1; app->fd = -1; app->print_fd = STDOUT_FILENO;
	app->environment = g_get_environ(); app->cols = 120; app->rows = 24; app->allowalt = 1; app->geometry_pending = TRUE;
	app->cwd = g_get_current_dir();
	apps = g_list_append(apps, app);
	return app;
}

static gboolean
draw_outline(GtkWidget *widget, cairo_t *cr, gpointer data)
{
	(void)data;
	/* Match the original client-side outline, above terminal and WebKit paint. */
	cairo_set_source_rgb(cr, 0.6, 0.6, 0.6);
	cairo_set_line_width(cr, 1);
	cairo_rectangle(cr, 0.5, 0.5, gtk_widget_get_allocated_width(widget) - 1,
	    gtk_widget_get_allocated_height(widget) - 1);
	cairo_stroke(cr);
	return FALSE;
}

static void
show_app(App *app)
{
	windows++;
	app->window = gtk_window_new(GTK_WINDOW_TOPLEVEL);
	gtk_window_set_title(GTK_WINDOW(app->window), app->title ? app->title : "Worminal preview");
	gtk_window_set_default_size(GTK_WINDOW(app->window), app->cols * 10 + 4, app->rows * 18 + 42);
	GtkWidget *box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
	GtkWidget *overlay = gtk_overlay_new();
	gtk_container_add(GTK_CONTAINER(app->window), overlay);
	gtk_container_add(GTK_CONTAINER(overlay), box);
	GtkWidget *outline = gtk_drawing_area_new();
	/* An input window here would swallow scroll events above the terminal. */
	gtk_widget_set_has_window(outline, FALSE);
	gtk_overlay_add_overlay(GTK_OVERLAY(overlay), outline);
	gtk_overlay_set_overlay_pass_through(GTK_OVERLAY(overlay), outline, TRUE);
	g_signal_connect(outline, "draw", G_CALLBACK(draw_outline), NULL);
	app->notebook = gtk_notebook_new();
	gtk_notebook_set_show_tabs(GTK_NOTEBOOK(app->notebook), FALSE);
	gtk_notebook_set_show_border(GTK_NOTEBOOK(app->notebook), FALSE);
	app->tabstrip = tabs_new(app);
	gtk_box_pack_start(GTK_BOX(box), app->tabstrip, FALSE, FALSE, 0);
	gtk_box_pack_start(GTK_BOX(box), app->notebook, TRUE, TRUE, 0);
	app->status = gtk_label_new("");
	gtk_widget_set_no_show_all(app->status, TRUE);
	gtk_box_pack_start(GTK_BOX(box), app->status, FALSE, FALSE, 0);
	g_signal_connect(app->window, "destroy", G_CALLBACK(destroyed), app);
	g_signal_connect(app->window, "key-press-event", G_CALLBACK(shortcut), app);
	/* GtkNotebook maps the new child in its default switch handler. Focus it
	 * afterwards so a hidden WebKit child cannot retain keyboard ownership. */
	g_signal_connect_after(app->notebook, "switch-page", G_CALLBACK(switched), app);
	connect_async(app);
	gtk_widget_show_all(app->window);
	app_startuptime("map-request");
}

void
app_new_window(App *previous)
{
	App *app = new_app();
	app->master = g_strdup(previous->master);
	app->font = g_strdup(previous->font);
	app->zoom = previous->zoom;
	g_strfreev(app->environment); app->environment = g_strdupv(previous->environment);
	app->print_fd = dup(previous->print_fd);
	show_app(app);
}

static void
usage(FILE *stream)
{
	fprintf(stream, "worminal " VERSION "\n\n"
	        "usage: worminal [--master user@host] [--surf URL | --attach]\n"
	        "       [-aiv] [-c class] [-n name] [-f font] [-g COLSxROWS] [-T title]\n"
	        "       [-o file] [-l line] [-e command ...]\n"
	        "       worminal --new-browser-tab [URL]  Replace this terminal with a browser\n"
	        "       worminal -h | --help       Show help and version\n"
	        "       worminal -v | --version    Show version\n");
}

static gboolean
launch_window(int argc, char **argv, char **env, const char *cwd, int output)
{
	App *app = new_app();
	g_strfreev(app->environment); app->environment = g_strdupv(env);
	g_free(app->cwd); app->cwd = g_strdup(cwd);
	app->print_fd = output;
	gboolean fixed = FALSE;
	char *class = NULL, *name = NULL;
	for (int i = 1; i < argc; i++) {
		const char *arg = argv[i];
		if (!strcmp(arg, "-v")) goto invalid;
		if (!strcmp(arg, "--attach")) { app->initial_attach = TRUE; continue; }
		if (!strcmp(arg, "-a")) { app->allowalt = 0; continue; }
		if (!strcmp(arg, "-i")) { fixed = TRUE; continue; }
		if (!strcmp(arg, "-e") || !strcmp(arg, "--") || *arg != '-') {
			if (*arg == '-') i++;
			app->command = g_strdupv(argv + i);
			break;
		}
		if (++i >= argc) goto invalid;
		char *value = argv[i];
		if (!strcmp(arg, "--master")) app->master = g_strdup(value);
		else if (!strcmp(arg, "--surf")) app->initial_url = g_strdup(value);
		else if (!strcmp(arg, "-T") || !strcmp(arg, "-t")) app->title = g_strdup(value);
		else if (!strcmp(arg, "-f")) app->font = g_strdup(value);
		else if (!strcmp(arg, "-o")) app->output = g_strdup(value);
		else if (!strcmp(arg, "-l")) app->line = g_strdup(value);
		else if (!strcmp(arg, "-c")) class = value;
		else if (!strcmp(arg, "-n")) name = value;
		else if (!strcmp(arg, "-g")) {
			if (sscanf(value, "%dx%d", &app->cols, &app->rows) != 2 ||
			    app->cols < 1 || app->cols > 400 || app->rows < 1 || app->rows > 200) goto invalid;
		} else goto invalid;
	}
	if ((!app->output || strcmp(app->output, "-")) && output != STDOUT_FILENO) {
		close(output); app->print_fd = STDOUT_FILENO;
	}
	if (class) gdk_set_program_class(class);
	show_app(app);
	if (fixed) gtk_window_set_resizable(GTK_WINDOW(app->window), FALSE);
	if (name) gtk_widget_set_name(app->window, name);
	return TRUE;
invalid:
	apps = g_list_remove(apps, app);
	app_unref(app);
	usage(stderr);
	return FALSE;
}

int
worminal_frontend_v1(int argc, char **argv, int listener)
{
	setlocale(LC_CTYPE, "");
	if (argc >= 2 && !strcmp(argv[1], "--new-browser-tab")) {
		if (argc > 3) { usage(stderr); return 2; }
		char daemon[4096];
		/* Exec preserves the calling PTY session and the daemon's command handling. */
		if (workspace_daemon_path(daemon, sizeof(daemon)))
			execl(daemon, daemon, "--new-browser-tab", argc == 3 ? argv[2] : "about:blank", (char *)NULL);
		perror("Could not run the workspace browser command");
		return 1;
	}
	if (argc == 2 && (!strcmp(argv[1], "-h") || !strcmp(argv[1], "--help"))) {
		usage(stdout);
		return 0;
	}
	if (argc == 2 && (!strcmp(argv[1], "-v") || !strcmp(argv[1], "--version"))) {
		g_print("worminal " VERSION "\n");
		return 0;
	}
	g_set_prgname("worminal");
	gtk_init(NULL, NULL);
	app_startuptime("gtk");
	guint source = windows_listen(listener, launch_window);
	char **env = g_get_environ();
	char *cwd = g_get_current_dir();
	gboolean okay = launch_window(argc, argv, env, cwd, STDOUT_FILENO);
	g_strfreev(env); g_free(cwd);
	if (okay) gtk_main();
	g_source_remove(source); close(listener);
	return okay ? 0 : 2;
}
