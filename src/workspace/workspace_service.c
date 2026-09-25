#define _GNU_SOURCE
#include <errno.h>
#include <limits.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/select.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <time.h>
#include <unistd.h>
#include <wchar.h>
#include "st.h"
#include "win.h"
#include "session_view.h"
#include "wire.h"
#include "workspace_socket.h"
#include WORMINAL_THEME_HEADER

char *utmp, *scroll, *stty_args = "stty raw pass8 nl -echo -iexten -cstopb 38400";
char *vtiden = "\033[?6c", *termname = "xterm-256color";
wchar_t *worddelimiters = L" ";
int allowaltscreen = 1, allowwindowops = 0;
unsigned int defaultfg = 258, defaultbg = 259, defaultcs = 256, tabspaces = 8;

typedef struct Client Client;
typedef struct Tab Tab;
struct Client {
	int fd, ready;
	int needs_resync;
	WireBuffer output;
	size_t sent;
	Client *next;
};
struct Tab {
	uint32_t id;
	TermSession *session;
	char *title, *initial_title, *directory, *clipboard;
	char **colors;
	uint32_t mode, cursor;
	Client *controller;
	Client *print_client;
	int pending_frame;
	Tab *next;
};

typedef struct {
	uint32_t cols, rows, allowalt, argc, envc;
	char *title, *cwd, *line, *output, *windowid;
	char **args, **env;
} Launch;

static Client *clients;
static Tab *tabs, *lasttab;
static uint32_t next_id = 1;
static uint32_t closed_id, fallback_id;
static int catalog_dirty;
static int running = 1;

static Tab *
tab_for_session(TermSession *session)
{
	for (Tab *tab = tabs; tab; tab = tab->next)
		if (tab->session == session)
			return tab;
	return NULL;
}

static Tab *
tab_for_id(uint32_t id)
{
	for (Tab *tab = tabs; tab; tab = tab->next)
		if (tab->id == id)
			return tab;
	return NULL;
}

static void
client_drop(Client *client)
{
	if (client->fd >= 0)
		close(client->fd);
	client->fd = -1;
	client->ready = 0;
	wire_buffer_free(&client->output);
	client->output = (WireBuffer){0};
	client->sent = 0;
	for (Tab *tab = tabs; tab; tab = tab->next)
		if (tab->controller == client)
			tab->controller = NULL;
	for (Tab *tab = tabs; tab; tab = tab->next)
		if (tab->print_client == client)
			tab->print_client = NULL;
}

static int
client_flush(Client *client)
{
	while (client->fd >= 0 && client->sent < client->output.len) {
		ssize_t count = send(client->fd, client->output.data + client->sent,
		                     client->output.len - client->sent, MSG_DONTWAIT | MSG_NOSIGNAL);
		if (count < 0 && errno == EINTR)
			continue;
		if (count < 0 && (errno == EAGAIN || errno == EWOULDBLOCK))
			return 1;
		if (count <= 0) {
			client_drop(client);
			return 0;
		}
		client->sent += count;
	}
	if (client->fd >= 0) {
		client->output.len = 0;
		client->sent = 0;
	}
	return client->fd >= 0;
}

static int
client_send(Client *client, uint32_t type, uint32_t tab, const WireBuffer *buffer)
{
	size_t length = buffer ? buffer->len : 0;
	if (client->fd < 0)
		return 0;
	if (client->sent) {
		memmove(client->output.data, client->output.data + client->sent,
		        client->output.len - client->sent);
		client->output.len -= client->sent;
		client->sent = 0;
	}
	if (client->output.len + sizeof(uint32_t) * 4 + length > 4u * 1024u * 1024u) {
		client_drop(client);
		return 0;
	}
	wire_append_packet(&client->output, type, tab, buffer ? buffer->data : NULL, length);
	return client_flush(client);
}

static void
take_control(Tab *tab, Client *client)
{
	if (tab->controller && tab->controller != client)
		client_send(tab->controller, WIRE_RELEASE, tab->id, NULL);
	tab->controller = client;
}

static void
service_print(const char *bytes, size_t length)
{
	Tab *tab = tab_for_session(tsessioncurrent());
	WireBuffer buffer = {0};
	if (!tab || !tab->print_client)
		return;
	wire_put_bytes(&buffer, bytes, length);
	client_send(tab->print_client, WIRE_PRINT, tab->id, &buffer);
	wire_buffer_free(&buffer);
}

static void
send_catalog(Client *client)
{
	WireBuffer buffer = {0};
	uint32_t count = 0;
	for (Tab *tab = tabs; tab; tab = tab->next)
		count++;
	wire_put_u32(&buffer, count);
	wire_put_u32(&buffer, closed_id);
	wire_put_u32(&buffer, fallback_id);
	for (Tab *tab = tabs; tab; tab = tab->next) {
		wire_put_u32(&buffer, tab->id);
		wire_put_u32(&buffer, tab->mode);
		wire_put_u32(&buffer, tab->cursor);
		wire_put_string(&buffer, tab->title);
		wire_put_string(&buffer, tab->directory);
		wire_put_u32(&buffer, sizeof(colorname) / sizeof(colorname[0]));
		for (size_t i = 0; i < sizeof(colorname) / sizeof(colorname[0]); i++)
			wire_put_string(&buffer, tab->colors && tab->colors[i] ? tab->colors[i] : colorname[i]);
	}
	client_send(client, WIRE_CATALOG, 0, &buffer);
	wire_buffer_free(&buffer);
}

static void
broadcast_catalog(void)
{
	if (!catalog_dirty)
		return;
	for (Client *client = clients; client; client = client->next)
		if (client->ready)
			send_catalog(client);
	catalog_dirty = 0;
	closed_id = fallback_id = 0;
}

static void
send_frame(Client *client, Tab *tab, int full)
{
	SessionFrame frame = tsessionframe(tab->session);
	WireBuffer buffer = {0};
	if (!client || client->fd < 0 || !client->ready)
		return;
	if (client->output.len - client->sent > 1024u * 1024u) {
		client->needs_resync = 1;
		return;
	}
	wire_put_u32(&buffer, frame.cols);
	wire_put_u32(&buffer, frame.rows);
	wire_put_u32(&buffer, frame.cursor_x);
	wire_put_u32(&buffer, frame.cursor_y);
	wire_put_u32(&buffer, frame.scroll);
	wire_put_u32(&buffer, frame.alternate);
	if (!client_send(client, WIRE_FRAME, tab->id, &buffer))
		goto done;
	wire_buffer_free(&buffer);
	buffer = (WireBuffer){0};
	for (int row = 0; row < frame.rows; row++) {
		if (!full && !tsessionrowdirty(tab->session, row))
			continue;
		Line line = tsessionviewline(tab->session, row);
		wire_put_u32(&buffer, row);
		for (int col = 0; col < frame.cols; col++) {
			wire_put_u32(&buffer, line[col].u);
			wire_put_u32(&buffer, line[col].mode);
			wire_put_u32(&buffer, line[col].fg);
			wire_put_u32(&buffer, line[col].bg);
		}
		if (!client_send(client, WIRE_ROW, tab->id, &buffer))
			goto done;
		buffer.len = 0;
	}
	client_send(client, WIRE_FRAME_FINISH, tab->id, NULL);
done:
	wire_buffer_free(&buffer);
}

static void
refresh_directory(Tab *tab)
{
	char path[64], directory[PATH_MAX];
	pid_t child = tsessionpid(tab->session);
	ssize_t length;
	if (child <= 0)
		return;
	snprintf(path, sizeof(path), "/proc/%ld/cwd", (long)child);
	length = readlink(path, directory, sizeof(directory) - 1);
	if (length < 0 || length >= (ssize_t)sizeof(directory) - 1)
		return;
	directory[length] = '\0';
	if (strcmp(tab->directory, directory) == 0)
		return;
	free(tab->directory);
	tab->directory = xstrdup(directory);
	catalog_dirty = 1;
}

static void
launch_free(Launch *launch)
{
	free(launch->title);
	free(launch->cwd);
	free(launch->line);
	free(launch->output);
	free(launch->windowid);
	for (uint32_t i = 0; i < launch->argc; i++)
		free(launch->args[i]);
	for (uint32_t i = 0; i < launch->envc; i++)
		free(launch->env[i]);
	free(launch->args);
	free(launch->env);
}

static int
launch_read(WirePacket *packet, Launch *launch)
{
	memset(launch, 0, sizeof(*launch));
	if (!wire_get_u32(packet, &launch->cols) || !wire_get_u32(packet, &launch->rows) ||
	    !wire_get_u32(packet, &launch->allowalt) || !wire_get_u32(packet, &launch->argc) ||
	    !wire_get_u32(packet, &launch->envc) || launch->cols < 1 || launch->cols > 400 ||
	    launch->rows < 1 || launch->rows > 200 || launch->argc > 1024 || launch->envc > 8192)
		return 0;
	launch->title = wire_get_string(packet);
	launch->cwd = wire_get_string(packet);
	launch->line = wire_get_string(packet);
	launch->output = wire_get_string(packet);
	launch->windowid = wire_get_string(packet);
	if (!launch->title || !launch->cwd || !launch->line || !launch->output ||
	    !launch->windowid)
		return 0;
	launch->args = calloc(launch->argc + 1, sizeof(char *));
	launch->env = calloc(launch->envc + 1, sizeof(char *));
	if (!launch->args || !launch->env)
		abort();
	for (uint32_t i = 0; i < launch->argc; i++)
		if (!(launch->args[i] = wire_get_string(packet)))
			return 0;
	for (uint32_t i = 0; i < launch->envc; i++)
		if (!(launch->env[i] = wire_get_string(packet)))
			return 0;
	return packet->pos == packet->len;
}

static Tab *
tab_start(Client *client, Launch *launch)
{
	Tab *tab = calloc(1, sizeof(*tab));
	const char *cwd = *launch->cwd ? launch->cwd : getenv("HOME");
	if (!tab)
		abort();
	if (!cwd || !*cwd)
		cwd = "/";
	tab->id = next_id++;
	tab->title = xstrdup(*launch->title ? launch->title : "Worminal");
	tab->initial_title = xstrdup(tab->title);
	tab->directory = xstrdup(cwd);
	tab->cursor = 2;
	tab->controller = client;
	if (strcmp(launch->output, "-") == 0)
		tab->print_client = client;
	tab->session = tsessionnew(launch->cols, launch->rows);
	tsessionallowalt(tab->session, launch->allowalt);
	if (lasttab)
		lasttab->next = tab;
	else
		tabs = tab;
	lasttab = tab;
	ttysetlaunch(cwd, launch->env, *launch->windowid ? launch->windowid : NULL);
	ttynew(*launch->line ? launch->line : NULL, "/bin/sh",
	       *launch->output ? launch->output : NULL,
	       launch->argc ? launch->args : NULL);
	ttysetlaunch(NULL, NULL, NULL);
	catalog_dirty = 1;
	return tab;
}

static void
tab_close(Tab *tab)
{
	Tab **slot = &tabs;
	Tab *preceding = NULL;
	while (*slot && *slot != tab) {
		preceding = *slot;
		slot = &(*slot)->next;
	}
	if (!*slot)
		return;
	closed_id = tab->id;
	fallback_id = tab->next ? tab->next->id : preceding ? preceding->id : 0;
	*slot = tab->next;
	if (lasttab == tab) {
		lasttab = tabs;
		while (lasttab && lasttab->next)
			lasttab = lasttab->next;
	}
	tsessionremove(tab->session);
	free(tab->title);
	free(tab->initial_title);
	free(tab->directory);
	free(tab->clipboard);
	if (tab->colors) {
		for (size_t i = 0; i < sizeof(colorname) / sizeof(colorname[0]); i++)
			free(tab->colors[i]);
		free(tab->colors);
	}
	free(tab);
	catalog_dirty = 1;
}

static void
client_message(Client *client, WirePacket *packet)
{
	Tab *tab = tab_for_id(packet->tab);
	uint32_t cols, rows, direction, amount;
	Launch launch;
	if (packet->type == WIRE_STOP) {
		running = 0;
		return;
	}
	if (packet->type == WIRE_HELLO || packet->type == WIRE_NEW) {
		if (!launch_read(packet, &launch)) {
			launch_free(&launch);
			client_drop(client);
			return;
		}
		client->ready = 1;
		tab = tab_start(client, &launch);
		launch_free(&launch);
		broadcast_catalog();
		send_frame(client, tab, 1);
		return;
	}
	if (!client->ready || !tab)
		return;
	switch (packet->type) {
	case WIRE_FOCUS:
		if (!wire_get_u32(packet, &cols) || !wire_get_u32(packet, &rows) ||
		    cols < 1 || cols > 400 || rows < 1 || rows > 200 || packet->pos != packet->len)
			break;
		take_control(tab, client);
		tsessionuse(tab->session);
		if (cols != (uint32_t)tsessionframe(tab->session).cols ||
		    rows != (uint32_t)tsessionframe(tab->session).rows) {
			tresize(cols, rows);
			ttyresize(0, 0);
		}
		send_frame(client, tab, 1);
		tab->pending_frame = 0;
		tsessioncleandirty(tab->session);
		break;
	case WIRE_INPUT:
		if (packet->len > 65536)
			break;
		take_control(tab, client);
		tsessionuse(tab->session);
		ttywrite((char *)packet->data, packet->len, 1);
		send_frame(client, tab, 0);
		tsessioncleandirty(tab->session);
		break;
	case WIRE_RELEASE:
		if (tab->controller == client)
			tab->controller = NULL;
		break;
	case WIRE_CLOSE:
		tab_close(tab);
		break;
	case WIRE_SCROLL:
		if (!wire_get_u32(packet, &direction) || !wire_get_u32(packet, &amount) ||
		    direction > 1 || packet->pos != packet->len)
			break;
		take_control(tab, client);
		tsessionuse(tab->session);
		Arg steps = {.i = (int32_t)amount};
		if (direction)
			kscrollup(&steps);
		else
			kscrolldown(&steps);
		send_frame(client, tab, 1);
		tsessioncleandirty(tab->session);
		break;
	default:
		break;
	}
}

static int
serve(void)
{
	int listener = workspace_listen();
	struct timespec last_frame = {0}, flush_delay = {.tv_nsec = 16000000};
	/* -o - follows the launching client's stdout across the service boundary. */
	tprinterhook = service_print;
	if (listener < 0)
		return 1;
	for (; running;) {
		fd_set readers, writers;
		int maxfd = listener;
		int pending = 0;
		FD_ZERO(&readers);
		FD_ZERO(&writers);
		FD_SET(listener, &readers);
		tsessionreap();
		for (Tab *tab = tabs, *next; tab; tab = next) {
			next = tab->next;
			if (tsessionfd(tab->session) < 0)
				tab_close(tab);
		}
		broadcast_catalog();
		for (Client *client = clients; client; client = client->next)
			if (client->fd >= 0 && client->needs_resync &&
			    client->output.len - client->sent < 65536) {
				client->needs_resync = 0;
				for (Tab *tab = tabs; tab; tab = tab->next)
					if (tab->controller == client)
						send_frame(client, tab, 1);
			}
		for (Client *client = clients; client; client = client->next)
			if (client->fd >= 0) {
				FD_SET(client->fd, &readers);
				if (client->sent < client->output.len)
					FD_SET(client->fd, &writers);
				if (client->fd > maxfd)
					maxfd = client->fd;
			}
		for (Tab *tab = tabs; tab; tab = tab->next) {
			pending |= tab->pending_frame;
			int fd = tsessionfd(tab->session);
			if (fd >= 0) {
				FD_SET(fd, &readers);
				if (fd > maxfd)
					maxfd = fd;
			}
		}
		if (pselect(maxfd + 1, &readers, &writers, NULL,
		            pending ? &flush_delay : NULL, NULL) < 0) {
			if (errno == EINTR)
				continue;
			return 1;
		}
		for (Tab *tab = tabs; tab; tab = tab->next) {
			int fd = tsessionfd(tab->session);
			if (fd < 0 || !FD_ISSET(fd, &readers))
				continue;
			tsessionuse(tab->session);
			ttyread();
			refresh_directory(tab);
			tab->pending_frame = 1;
		}
		if (FD_ISSET(listener, &readers)) {
			int fd = accept4(listener, NULL, NULL, SOCK_CLOEXEC);
			struct { pid_t pid; uid_t uid; gid_t gid; } peer;
			socklen_t length = sizeof(peer);
			if (fd >= 0 && (getsockopt(fd, SOL_SOCKET, SO_PEERCRED, &peer, &length) < 0 ||
			                peer.uid != getuid())) {
				close(fd);
				fd = -1;
			}
			if (fd >= 0) {
				Client *client = calloc(1, sizeof(*client));
				struct timeval timeout = {.tv_sec = 2};
				setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout));
				client->fd = fd;
				client->next = clients;
				clients = client;
			}
		}
		for (Client *client = clients; client; client = client->next) {
			if (client->fd < 0 || !FD_ISSET(client->fd, &readers))
				continue;
			WirePacket packet;
			if (wire_read(client->fd, &packet)) {
				client_message(client, &packet);
				wire_packet_free(&packet);
			} else {
				client_drop(client);
			}
		}
		for (Client *client = clients; client; client = client->next)
			if (client->fd >= 0 && FD_ISSET(client->fd, &writers))
				client_flush(client);
		struct timespec now;
		clock_gettime(CLOCK_MONOTONIC, &now);
		if (!last_frame.tv_sec ||
		    (now.tv_sec - last_frame.tv_sec) * 1000000000LL +
		    now.tv_nsec - last_frame.tv_nsec >= 16000000) {
			for (Tab *tab = tabs; tab; tab = tab->next)
				if (tab->pending_frame) {
					send_frame(tab->controller, tab, 0);
					tsessioncleandirty(tab->session);
					tab->pending_frame = 0;
				}
			last_frame = now;
		}
		broadcast_catalog();
		Client **slot = &clients;
		while (*slot) {
			if ((*slot)->fd >= 0) {
				slot = &(*slot)->next;
				continue;
			}
			Client *dead = *slot;
			*slot = dead->next;
			free(dead);
		}
	}
	tsessionhangupall();
	close(listener);
	return 0;
}

static int
bridge(void)
{
	int fd = workspace_ensure();
	char buffer[16384];
	if (fd < 0)
		return 1;
	for (;;) {
		fd_set readers;
		FD_ZERO(&readers);
		FD_SET(0, &readers);
		FD_SET(fd, &readers);
		if (select(fd + 1, &readers, NULL, NULL, NULL) < 0) {
			if (errno == EINTR)
				continue;
			break;
		}
		int source = FD_ISSET(fd, &readers) ? fd : 0;
		int target = source == fd ? 1 : fd;
		ssize_t count = read(source, buffer, sizeof(buffer));
		if (count <= 0)
			break;
		for (ssize_t done = 0; done < count;) {
			ssize_t written = write(target, buffer + done, count - done);
			if (written < 0 && errno == EINTR)
				continue;
			if (written <= 0)
				goto out;
			done += written;
		}
	}
out:
	close(fd);
	return 0;
}

int
main(int argc, char **argv)
{
	if (argc == 2 && strcmp(argv[1], "--serve") == 0)
		return serve();
	if (argc == 2 && strcmp(argv[1], "--bridge") == 0)
		return bridge();
	if (argc == 2 && strcmp(argv[1], "--stop") == 0) {
		int fd = workspace_connect();
		if (fd < 0)
			return 1;
		int okay = wire_write(fd, WIRE_STOP, 0, NULL, 0);
		close(fd);
		return okay ? 0 : 1;
	}
	fprintf(stderr, "usage: worminald --serve|--bridge|--stop\n");
	return 2;
}

/* st's terminal parser reports changes through these callbacks. */
void xbell(void) {}
void xclipcopy(void)
{
	Tab *tab = tab_for_session(tsessioncurrent());
	WireBuffer buffer = {0};
	if (!tab || !tab->controller || !tab->clipboard)
		return;
	wire_put_string(&buffer, tab->clipboard);
	client_send(tab->controller, WIRE_CLIPBOARD, tab->id, &buffer);
	wire_buffer_free(&buffer);
}
void xsetsel(char *value)
{
	Tab *tab = tab_for_session(tsessioncurrent());
	if (tab) {
		free(tab->clipboard);
		tab->clipboard = value;
	} else {
		free(value);
	}
}
void xsettitle(char *value)
{
	Tab *tab = tab_for_session(tsessioncurrent());
	if (!tab)
		return;
	free(tab->title);
	tab->title = xstrdup(value && *value ? value : tab->initial_title);
	catalog_dirty = 1;
}
void xseticontitle(char *value) { (void)value; }
void xsetmode(int set, unsigned int flags)
{
	Tab *tab = tab_for_session(tsessioncurrent());
	if (!tab)
		return;
	if (set)
		tab->mode |= flags;
	else
		tab->mode &= ~flags;
	catalog_dirty = 1;
}
void xsetpointermotion(int set) { (void)set; }
int xsetcursor(int cursor)
{
	Tab *tab = tab_for_session(tsessioncurrent());
	if (cursor < 0 || cursor > 7)
		return 1;
	if (tab) {
		tab->cursor = cursor;
		catalog_dirty = 1;
	}
	return 0;
}
int xsetcolorname(int index, const char *name)
{
	Tab *tab = tab_for_session(tsessioncurrent());
	if (!tab || index < 0 || index >= (int)(sizeof(colorname) / sizeof(colorname[0])))
		return 1;
	if (!tab->colors)
		tab->colors = calloc(sizeof(colorname) / sizeof(colorname[0]), sizeof(char *));
	free(tab->colors[index]);
	tab->colors[index] = name ? xstrdup(name) : NULL;
	catalog_dirty = 1;
	return 0;
}
int xgetcolor(int index, unsigned char *red, unsigned char *green, unsigned char *blue)
{
	Tab *tab = tab_for_session(tsessioncurrent());
	const char *name;
	unsigned int r, g, b;
	if (index < 0 || index >= (int)(sizeof(colorname) / sizeof(colorname[0])))
		return 1;
	name = tab && tab->colors && tab->colors[index] ? tab->colors[index] : colorname[index];
	if (!name || sscanf(name, "#%2x%2x%2x", &r, &g, &b) != 3)
		return 1;
	*red = r;
	*green = g;
	*blue = b;
	return 0;
}
void xloadcols(void) { catalog_dirty = 1; }
int xstartdraw(void) { return 0; }
int xoverlap(void) { return 0; }
void xdrawline(Line line, int x1, int y1, int x2, int pass)
{
	(void)line; (void)x1; (void)y1; (void)x2; (void)pass;
}
void xdrawcursor(int x, int y, Glyph glyph, int oldx, int oldy, Glyph old)
{
	(void)x; (void)y; (void)glyph; (void)oldx; (void)oldy; (void)old;
}
void xfinishdraw(void) {}
void xximspot(int x, int y) { (void)x; (void)y; }
