#ifndef WORMINAL_PREVIEW_H
#define WORMINAL_PREVIEW_H

#include <gtk/gtk.h>
#include <stdint.h>
#include "st.h"
#include "session_view.h"
#include "wire.h"

typedef struct App App;
typedef struct Page Page;
typedef struct Browser Browser;
typedef struct Terminal Terminal;

struct Page {
	App *app;
	uint32_t id, mode, cursor;
	char *url, *directory, *title;
	GtkWidget *widget, *label, *placeholder;
	Terminal *terminal;
	Browser *browser;
	gboolean seen, live;
	GdkRGBA colors[270];
};
struct App {
	GtkWidget *window, *notebook, *status, *tabstrip;
	GList *pages;
	Page *active;
	char *master, *title, *cwd, *line, *output, *font;
	char **command, **environment;
	int cols, rows, allowalt, refs;
	char *initial_url;
	gboolean initial_attach, geometry_pending;
	double zoom;
	int fd, print_fd;
	pid_t connecting_child;
	guint reader, writer, reconnect;
	WireReader input;
	WireBuffer output_queue;
	size_t sent;
	guint pending;
	GQueue early_keys;
	uint32_t created_id;
	gboolean attached, closing, syncing;
	/* The font pattern is immutable per window; zoom invalidates these metrics. */
	PangoFontDescription *cached_font;
	double cached_font_zoom;
	int font_cw, font_ch, font_height, font_ascent;
	gint64 last_selection;
};

void tabs_refresh(App *);
GtkWidget *tabs_new(App *);
PangoFontDescription *terminal_font(App *, PangoLayout *, int *, int *, int *, int *);

void app_send(App *, uint32_t, uint32_t, const void *, size_t);
void app_terminal(App *, gboolean);
void app_browser(App *, const char *);
void app_focus(Page *);
void app_resize(Page *);
void app_close(Page *);
void app_select(Page *);
void app_status(App *, const char *);
void app_startuptime(const char *);
void app_page_title(Page *, const char *);
void app_new_window(App *);
void page_free(Page *);

Terminal *terminal_new(Page *);
void terminal_free(Terminal *);
void terminal_packet(Terminal *, WirePacket *);
void terminal_zoom(Terminal *);
void terminal_copy(Terminal *, GdkAtom);
void terminal_paste(Terminal *, GdkAtom);
gboolean terminal_key(Terminal *, GdkEventKey *);
gboolean terminal_palette(Page *, uint32_t, const char *);
void terminal_configure(Terminal *, uint32_t, uint32_t, gboolean);
void terminal_select(Terminal *);
void terminal_dimensions(Terminal *, int *, int *);

G_GNUC_INTERNAL Browser *browser_new(App *);
G_GNUC_INTERNAL GtkWidget *browser_widget(Browser *);
G_GNUC_INTERNAL GtkWidget *browser_view(Browser *);
G_GNUC_INTERNAL void browser_bind(Browser *, Page *, const char *);
G_GNUC_INTERNAL gboolean browser_shortcut(Page *, guint, guint);
G_GNUC_INTERNAL void browser_zoom(Browser *, double);
G_GNUC_INTERNAL void browser_free(Browser *);
#endif
