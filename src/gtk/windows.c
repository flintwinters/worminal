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

int
windows_forward(int argc, char **argv, int *listener)
{
	char *key = g_strdup_printf("%s:%s:%s:%s", g_getenv("GDK_BACKEND") ?: "",
	    g_getenv("WAYLAND_DISPLAY") ?: "", g_getenv("DISPLAY") ?: "",
	    g_getenv("WORMINAL_SHARED_SOCKET_SCOPE") ?: "");
	char *hash = g_compute_checksum_for_string(G_CHECKSUM_SHA256, key, -1);
	struct sockaddr_un address = {.sun_family = AF_UNIX};
	int size = g_snprintf(address.sun_path + 1, sizeof(address.sun_path) - 1,
	    "worminal-preview-ui-%lu-%.48s", (unsigned long)getuid(), hash);
	g_free(key); g_free(hash);
	socklen_t length = offsetof(struct sockaddr_un, sun_path) + 1 + size;
	int fd = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
	if (fd < 0) return -1;
	if (!bind(fd, (struct sockaddr *)&address, length)) {
		if (listen(fd, 16) < 0) { close(fd); return -1; }
		*listener = fd;
		return 0;
	}
	close(fd);
	fd = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
	if (fd < 0) return -1;
	if (connect(fd, (struct sockaddr *)&address, length) < 0) { close(fd); return -1; }
	char byte = 0, control[CMSG_SPACE(sizeof(int))] = {0};
	struct iovec iov = {&byte, 1};
	struct msghdr message = {.msg_iov = &iov, .msg_iovlen = 1,
	    .msg_control = control, .msg_controllen = sizeof(control)};
	struct cmsghdr *header = CMSG_FIRSTHDR(&message);
	header->cmsg_level = SOL_SOCKET; header->cmsg_type = SCM_RIGHTS;
	header->cmsg_len = CMSG_LEN(sizeof(int));
	int output = STDOUT_FILENO;
	memcpy(CMSG_DATA(header), &output, sizeof(output));
	WireBuffer buffer = {0};
	char *cwd = g_get_current_dir();
	wire_put_string(&buffer, cwd); g_free(cwd);
	wire_put_u32(&buffer, argc);
	for (int i = 0; i < argc; i++) wire_put_string(&buffer, argv[i]);
	char **env = g_get_environ();
	wire_put_u32(&buffer, g_strv_length(env));
	for (char **entry = env; *entry; entry++) wire_put_string(&buffer, *entry);
	g_strfreev(env);
	struct timeval timeout = {.tv_sec = 5};
	setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout));
	setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &timeout, sizeof(timeout));
	int okay = sendmsg(fd, &message, MSG_NOSIGNAL) == 1 &&
	    wire_write(fd, WIRE_HELLO, 0, buffer.data, buffer.len) && recv(fd, &byte, 1, 0) == 1;
	wire_buffer_free(&buffer); close(fd);
	return okay ? (byte == 0 ? 1 : -1) : -1;
}
