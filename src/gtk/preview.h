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
	GtkWidget *widget, *label;
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
	char **command;
	int cols, rows, allowalt, refs;
	char *initial_url;
	gboolean initial_attach, geometry_pending;
	double zoom;
	int fd;
	pid_t connecting_child;
	guint reader, writer, reconnect;
	WireReader input;
	WireBuffer output_queue;
	size_t sent;
	guint pending;
	GQueue early_keys;
	uint32_t created_id;
	gboolean attached, closing, syncing;
};

void tabs_refresh(App *);
GtkWidget *tabs_new(App *);
PangoFontDescription *terminal_font(App *, PangoLayout *, int *, int *, int *, int *);

void app_send(App *, uint32_t, uint32_t, const void *, size_t);
void app_terminal(App *, gboolean);
void app_browser(App *, const char *);
void app_location(App *);
void app_focus(Page *);
void app_resize(Page *);
void app_close(Page *);
void app_status(App *, const char *);
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
void terminal_palette(Page *, uint32_t, const char *);
void terminal_dimensions(Terminal *, int *, int *);

Browser *browser_new(App *);
GtkWidget *browser_widget(Browser *);
GtkWidget *browser_view(Browser *);
void browser_bind(Browser *, Page *, const char *);
void browser_location(Browser *);
void browser_zoom(Browser *, double);
void browser_free(Browser *);
#endif
