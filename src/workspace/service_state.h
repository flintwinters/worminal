/* Service-owned records shared with the local handoff transaction. */
#ifndef WORMINAL_SERVICE_STATE_H
#define WORMINAL_SERVICE_STATE_H
#include "st.h"
#include "wire.h"
typedef struct Client Client;
typedef struct Tab Tab;
struct Client {
	int fd, ready;
	pid_t pid;
	int needs_resync;
	WireReader input;
	WireBuffer output;
	size_t sent;
	Client *next;
};
struct Tab {
	uint32_t id;
	TermSession *session;
	WireBuffer input;
	size_t input_sent;
	WireBuffer screen; /* Encoded cells last queued to the controller, not PTY dirty state. */
	int screen_cols;
	char *title, *initial_title, *directory, *clipboard;
	char *url; /* Non-NULL marks a browser record: no terminal or PTY. */
	char **colors;
	uint32_t mode, cursor;
	Client *controller;
	Client *print_client;
	int pending_frame;
	int input_frame;
	Tab *next;
};

static inline void
tab_release_control(Tab *tab)
{
	tab->controller = NULL;
	wire_buffer_free(&tab->screen);
	tab->screen = (WireBuffer){0};
}

#endif
