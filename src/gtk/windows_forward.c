/* Forward a launch without loading the GTK frontend or its graphics libraries. */
#define _GNU_SOURCE
#include <stddef.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>
#include "wire.h"
#include "windows.h"

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
