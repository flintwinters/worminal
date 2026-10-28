/* Explicit same-user, local PTY ownership transfer. The donor pauses parsing
 * during prepare/commit; the receiver never reads a PTY before commit. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <unistd.h>
#include "workspace_handoff.h"
#include "workspace_socket.h"
#include "session_transfer.h"
#include "st_state.h"
#include WORMINAL_THEME_HEADER

#define MAX_TABS 100
#define MAX_FDS (1 + 2 * MAX_TABS)
#define SNAPSHOT_LIMIT (512u * 1024u * 1024u)
#define SEALS (F_SEAL_WRITE | F_SEAL_GROW | F_SEAL_SHRINK | F_SEAL_SEAL)

static int
local_socket(const char *name, int listening)
{
	struct sockaddr_un address = {.sun_family = AF_UNIX};
	int length, fd;
	if (!name || !*name || strlen(name) > 60) return -1;
	length = snprintf(address.sun_path + 1, sizeof(address.sun_path) - 1,
	                  "worminal-handoff-%lu-%s", (unsigned long)getuid(), name);
	if (length < 0 || length >= (int)sizeof(address.sun_path) - 1) return -1;
	fd = socket(AF_UNIX, SOCK_SEQPACKET | SOCK_CLOEXEC, 0);
	if (fd < 0) return -1;
	socklen_t size = offsetof(struct sockaddr_un, sun_path) + 1 + length;
	if (listening ? bind(fd, (struct sockaddr *)&address, size) < 0 || listen(fd, 1) < 0 :
	    connect(fd, (struct sockaddr *)&address, size) < 0) {
		close(fd); return -1;
	}
	return fd;
}

int handin_listen(const char *name) { return local_socket(name, 1); }

static int
authenticate(int fd)
{
	struct ucred peer;
	socklen_t size = sizeof(peer);
	struct timeval timeout = {.tv_sec = 5};
	return getsockopt(fd, SOL_SOCKET, SO_PEERCRED, &peer, &size) == 0 && peer.uid == getuid() &&
	    setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout)) == 0 &&
	    setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &timeout, sizeof(timeout)) == 0;
}

static int
io(int fd, void *data, size_t size, int writing)
{
	char *p = data;
	while (size) {
		ssize_t n = writing ? write(fd, p, size) : read(fd, p, size);
		if (n < 0 && errno == EINTR) continue;
		if (n <= 0) return 0;
		p += n; size -= n;
	}
	return 1;
}

static int
marker(int fd, char expected, int writing)
{
	char value = expected;
	return writing ? send(fd, &value, 1, MSG_NOSIGNAL) == 1 :
	    recv(fd, &value, 1, 0) == 1 && value == expected;
}

static int
bundle(int fd, int *fds, int count, int writing)
{
	char byte = 'H', control[CMSG_SPACE(MAX_FDS * sizeof(int))] = {0};
	struct iovec iov = {.iov_base = &byte, .iov_len = 1};
	struct msghdr msg = {.msg_iov = &iov, .msg_iovlen = 1,
	    .msg_control = control, .msg_controllen = sizeof(control)};
	if (writing) {
		msg.msg_controllen = CMSG_SPACE(count * sizeof(int));
		struct cmsghdr *c = CMSG_FIRSTHDR(&msg);
		c->cmsg_level = SOL_SOCKET; c->cmsg_type = SCM_RIGHTS;
		c->cmsg_len = CMSG_LEN(count * sizeof(int));
		memcpy(CMSG_DATA(c), fds, count * sizeof(int));
		return sendmsg(fd, &msg, MSG_NOSIGNAL) == 1;
	}
	if (recvmsg(fd, &msg, MSG_CMSG_CLOEXEC) != 1) return 0;
	int received = 0;
	for (struct cmsghdr *c = CMSG_FIRSTHDR(&msg); c; c = CMSG_NXTHDR(&msg, c))
		if (c->cmsg_level == SOL_SOCKET && c->cmsg_type == SCM_RIGHTS) {
			size_t n = (c->cmsg_len - CMSG_LEN(0)) / sizeof(int);
			int *values = (int *)CMSG_DATA(c);
			for (size_t i = 0; i < n; i++) {
				if (received < MAX_FDS) fds[received++] = values[i];
				else close(values[i]);
			}
		}
	if (byte != 'H' || (msg.msg_flags & (MSG_CTRUNC | MSG_TRUNC))) {
		for (int i = 0; i < received; i++) close(fds[i]);
		return 0;
	}
	return received;
}

static int
export_tab(Tab *tab, int fd)
{
	WireBuffer b = {0};
	const char *values[] = {tab->title, tab->initial_title, tab->directory, tab->clipboard, tab->url};
	size_t total = 7 * sizeof(uint32_t) + tab->input.len - tab->input_sent;
	for (size_t i = 0; i < LEN(values); i++) total += 4 + (values[i] ? strlen(values[i]) : 0);
	for (size_t i = 0; i < LEN(colorname); i++)
		total += 4 + (tab->colors && tab->colors[i] ? strlen(tab->colors[i]) : 0);
	if (total > WIRE_MAX_BYTES) return 0;
	wire_put_u32(&b, tab->url != NULL);
	wire_put_u32(&b, tab->mode); wire_put_u32(&b, tab->cursor);
	wire_put_u32(&b, tab->session ? tsessionpid(tab->session) : 0);
	wire_put_string(&b, tab->title); wire_put_string(&b, tab->initial_title);
	wire_put_string(&b, tab->directory); wire_put_string(&b, tab->clipboard);
	wire_put_string(&b, tab->url);
	wire_put_u32(&b, LEN(colorname));
	for (size_t i = 0; i < LEN(colorname); i++)
		wire_put_string(&b, tab->colors ? tab->colors[i] : NULL);
	wire_put_u32(&b, tab->input.len - tab->input_sent);
	wire_put_bytes(&b, tab->input.data ? tab->input.data + tab->input_sent : NULL,
	               tab->input.len - tab->input_sent);
	uint32_t size = b.len;
	int okay = size <= WIRE_MAX_BYTES && io(fd, &size, sizeof(size), 1) && io(fd, b.data, size, 1);
	wire_buffer_free(&b);
	return okay && (!tab->session || tsessionexport(tab->session, fd));
}

static void
free_tab(Tab *tab)
{
	tab_release_control(tab);
	if (tab->session) tsessionremove(tab->session);
	free(tab->title); free(tab->initial_title); free(tab->directory);
	free(tab->clipboard); free(tab->url);
	if (tab->colors) {
		for (size_t i = 0; i < LEN(colorname); i++) free(tab->colors[i]);
		free(tab->colors);
	}
	wire_buffer_free(&tab->input);
	free(tab);
}

static Tab *
import_tab(int fd, pid_t *pid)
{
	uint32_t size, browser, child, colors, input;
	WirePacket p = {0};
	Tab *tab = calloc(1, sizeof(*tab));
	if (!tab) return NULL;
	if (!io(fd, &size, sizeof(size), 0) || size > WIRE_MAX_BYTES) goto fail;
	p.data = xmalloc(size); p.len = size;
	if (!io(fd, p.data, size, 0) || !wire_get_u32(&p, &browser) || browser > 1 ||
	    !wire_get_u32(&p, &tab->mode) || !wire_get_u32(&p, &tab->cursor) || tab->cursor > 7 ||
	    !wire_get_u32(&p, &child) || child > INT32_MAX) goto fail;
#ifndef WORMINAL_PREVIEW_PROTOCOL
	if (browser) goto fail; /* X11 cannot represent browser records. Leave the donor intact. */
#endif
	*pid = child;
	tab->title = wire_get_string(&p); tab->initial_title = wire_get_string(&p);
	tab->directory = wire_get_string(&p); tab->clipboard = wire_get_string(&p);
	tab->url = wire_get_string(&p);
	if (!tab->title || !tab->initial_title || !tab->directory || !tab->clipboard || !tab->url ||
	    !wire_get_u32(&p, &colors) || colors != LEN(colorname)) goto fail;
	if (!browser) { free(tab->url); tab->url = NULL; }
	tab->colors = calloc(colors, sizeof(char *));
	if (!tab->colors) goto fail;
	for (size_t i = 0; i < colors; i++) {
		char *color = wire_get_string(&p);
		if (!color) goto fail;
		if (*color) tab->colors[i] = color; else free(color);
	}
	if (!wire_get_u32(&p, &input) || input > 1024 * 1024 || input != p.len - p.pos) goto fail;
	wire_put_bytes(&tab->input, p.data + p.pos, input);
	if (!browser && !(tab->session = tsessionimport(fd))) goto fail;
	wire_packet_free(&p);
	return tab;
fail:
	wire_packet_free(&p); free_tab(tab);
	return NULL;
}

int
handoff_send(const char *name, Tab *tabs, void (*remove_tab)(Tab *))
{
	int peer = -1, fd = -1, okay = 0, fds[MAX_FDS], nfds = 1;
	uint32_t header[] = {0x57484f31, 1, 0};
	for (Tab *t = tabs; t; t = t->next) if (++header[2] > MAX_TABS) goto done;
	if (!header[2]) goto done;
	peer = local_socket(name, 0);
	if (peer < 0 || !authenticate(peer)) goto done;
	fd = memfd_create("worminal-handoff", MFD_CLOEXEC | MFD_ALLOW_SEALING);
	if (fd < 0 || !io(fd, header, sizeof(header), 1)) goto done;
	fds[0] = fd;
	for (Tab *t = tabs; t; t = t->next) {
		if (!export_tab(t, fd) || lseek(fd, 0, SEEK_CUR) > SNAPSHOT_LIMIT) goto done;
		if (t->session) {
			fds[nfds++] = tsessionfd(t->session);
			fds[nfds++] = t->session->output_fd < 0 ? STDOUT_FILENO : t->session->output_fd;
		}
	}
	if (fcntl(fd, F_ADD_SEALS, SEALS) < 0 || lseek(fd, 0, SEEK_SET) < 0 ||
	    !bundle(peer, fds, nfds, 1) || !marker(peer, 'R', 0) || !marker(peer, 'C', 1)) goto done;
	/* Commit was delivered: ownership is irrevocably the receiver's even if its
	 * acknowledgement is lost. Never resume donor reads after this point. */
	for (Tab *t = tabs, *next; t; t = next) {
		next = t->next;
		if (t->session) tsessiondisown(t->session);
		remove_tab(t);
	}
	okay = 1;
	if (!marker(peer, 'A', 0)) fputs("handoff committed; receiver acknowledgement missing\n", stderr);
done:
	if (fd >= 0) close(fd);
	if (peer >= 0) close(peer);
	return okay;
}

int
handin_accept(int listener, Tab **tabs, Tab **last, uint32_t *next_id)
{
	int peer = accept4(listener, NULL, NULL, SOCK_CLOEXEC), fds[MAX_FDS], nfds = 0, okay = 0;
	Tab *staged = NULL, *tail = NULL;
	pid_t pids[MAX_TABS];
	uint32_t header[3];
	struct stat st;
	int seals;
	if (peer < 0 || !authenticate(peer) || !(nfds = bundle(peer, fds, 0, 0))) goto done;
	seals = fcntl(fds[0], F_GET_SEALS);
	if (fstat(fds[0], &st) < 0 || !S_ISREG(st.st_mode) || st.st_size > SNAPSHOT_LIMIT ||
	    seals < 0 || (seals & SEALS) != SEALS || lseek(fds[0], 0, SEEK_SET) < 0 ||
	    !io(fds[0], header, sizeof(header), 0) || header[0] != 0x57484f31 || header[1] != 1 ||
	    !header[2] || header[2] > MAX_TABS || *next_id > UINT32_MAX - header[2]) goto done;
	int expected = 1;
	for (uint32_t i = 0; i < header[2]; i++) {
		Tab *t = import_tab(fds[0], &pids[i]);
		if (!t) goto done;
		t->id = *next_id + i;
		if (tail) tail->next = t; else staged = t;
		tail = t;
		if (t->session) {
			if (!tsessionprepare(t->session, pids[i])) goto done;
			if (expected + 2 > nfds || fds[expected] >= FD_SETSIZE || !isatty(fds[expected]) ||
			    fcntl(fds[expected], F_GETFL) < 0 ||
			    !(fcntl(fds[expected], F_GETFL) & O_NONBLOCK)) goto done;
			expected += 2;
		}
	}
	if (expected != nfds || lseek(fds[0], 0, SEEK_CUR) != st.st_size ||
	    !marker(peer, 'R', 1) || !marker(peer, 'C', 0)) goto done;
	int index = 0, n = 1;
	for (Tab *t = staged; t; t = t->next, index++)
		if (t->session) {
			tsessionadopt(t->session, fds[n], fds[n + 1], pids[index]);
			fds[n] = fds[n + 1] = -1; n += 2;
		}
	if (*last) (*last)->next = staged; else *tabs = staged;
	*last = tail; *next_id += header[2]; staged = NULL;
	okay = 1;
	marker(peer, 'A', 1);
done:
	while (staged) { Tab *next = staged->next; free_tab(staged); staged = next; }
	for (int i = 0; i < nfds; i++) if (fds[i] >= 0) close(fds[i]);
	if (peer >= 0) close(peer);
	return okay;
}

int
handoff_command(const char *name, int receiving)
{
	/* Never auto-spawn here: an old daemon must fail without touching sessions. */
	int fd = workspace_connect(), okay = 0;
	WireBuffer b = {0};
	WirePacket reply = {0};
	if (fd < 0 || !authenticate(fd)) goto done;
	wire_put_string(&b, name);
	if (!wire_write(fd, receiving ? WIRE_HANDIN : WIRE_HANDOFF, 0, b.data, b.len) ||
	    !wire_read(fd, &reply)) goto done;
	okay = reply.type == WIRE_COMMAND_DONE;
	if (!okay) {
		char *error = wire_get_string(&reply);
		if (error) { fprintf(stderr, "%s\n", error); free(error); }
	}
done:
	if (!okay) fputs("Handoff unavailable or rejected; no automatic daemon restart was attempted.\n", stderr);
	wire_packet_free(&reply); wire_buffer_free(&b);
	if (fd >= 0) close(fd);
	return okay ? 0 : 1;
}
