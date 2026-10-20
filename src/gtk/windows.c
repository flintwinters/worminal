/* One GTK owner per display/workspace permits live WebKit views to move between
 * windows, including windows requested by independently launched executables. */
#define _GNU_SOURCE
#include <errno.h>
#include <stddef.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>
#include <glib-unix.h>
#include "wire.h"
#include "windows.h"

typedef struct {
	int output;
	WireReader reader;
	WindowLaunch launch;
} Request;
static WindowLaunch window_launcher;

static char **
read_strings(WirePacket *packet)
{
	uint32_t count;
	if (!wire_get_u32(packet, &count) || count > 4096) return NULL;
	char **values = g_new0(char *, count + 1);
	for (uint32_t i = 0; i < count; i++) {
		if (!(values[i] = wire_get_string(packet))) { g_strfreev(values); return NULL; }
	}
	return values;
}

static gboolean
request_received(int fd, GIOCondition condition, gpointer data)
{
	Request *request = data;
	if (request->output < 0) {
		char byte, control[CMSG_SPACE(sizeof(int))];
		struct iovec iov = {&byte, 1};
		struct msghdr message = {.msg_iov = &iov, .msg_iovlen = 1,
		    .msg_control = control, .msg_controllen = sizeof(control)};
		ssize_t count = recvmsg(fd, &message, MSG_DONTWAIT | MSG_CMSG_CLOEXEC);
		if (count < 0 && (errno == EAGAIN || errno == EINTR)) return G_SOURCE_CONTINUE;
		if (count != 1) goto done;
		for (struct cmsghdr *header = CMSG_FIRSTHDR(&message); header; header = CMSG_NXTHDR(&message, header)) {
			if (header->cmsg_level != SOL_SOCKET || header->cmsg_type != SCM_RIGHTS) continue;
			size_t descriptors = (header->cmsg_len - CMSG_LEN(0)) / sizeof(int);
			for (size_t i = 0; i < descriptors; i++) {
				int received;
				memcpy(&received, CMSG_DATA(header) + i * sizeof(int), sizeof(int));
				if (request->output < 0) request->output = received;
				else { close(received); message.msg_flags |= MSG_CTRUNC; }
			}
		}
		if (request->output < 0 || (message.msg_flags & MSG_CTRUNC)) goto done;
	}
	WirePacket packet;
	int result = wire_tryread(fd, &request->reader, &packet);
	if (!result && !(condition & (G_IO_HUP | G_IO_ERR))) return G_SOURCE_CONTINUE;
	if (result > 0) {
		char *cwd = wire_get_string(&packet);
		char **args = read_strings(&packet), **env = read_strings(&packet);
		char status = 2;
		if (packet.type == WIRE_HELLO && cwd && args && args[0] && env && packet.pos == packet.len) {
			status = request->launch(g_strv_length(args), args, env, cwd, request->output) ? 0 : 2;
			request->output = -1; /* The new window owns the descriptor, even on parse failure. */
		}
		ssize_t ignored = send(fd, &status, 1, MSG_NOSIGNAL); (void)ignored;
		g_free(cwd); g_strfreev(args); g_strfreev(env); wire_packet_free(&packet);
	}
 done:
	close(fd);
	if (request->output >= 0) close(request->output);
	wire_reader_free(&request->reader);
	g_free(request);
	return G_SOURCE_REMOVE;
}

static gboolean
window_requested(int listener, GIOCondition condition, gpointer data)
{
	(void)condition; (void)data;
	int fd = accept4(listener, NULL, NULL, SOCK_CLOEXEC | SOCK_NONBLOCK);
	if (fd < 0) return G_SOURCE_CONTINUE;
	struct ucred peer;
	socklen_t length = sizeof(peer);
	if (getsockopt(fd, SOL_SOCKET, SO_PEERCRED, &peer, &length) < 0 || peer.uid != getuid()) {
		close(fd); return G_SOURCE_CONTINUE;
	}
	Request *request = g_new0(Request, 1);
	request->output = -1; request->launch = window_launcher;
	g_unix_fd_add(fd, G_IO_IN | G_IO_HUP | G_IO_ERR, request_received, request);
	return G_SOURCE_CONTINUE;
}

guint
windows_listen(int listener, WindowLaunch launch)
{
	window_launcher = launch;
	return g_unix_fd_add(listener, G_IO_IN, window_requested, NULL);
}
