#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>
#include "workspace_socket.h"

static socklen_t
workspace_address(struct sockaddr_un *address)
{
	const char *scope = getenv("WORMINAL_SHARED_SOCKET_SCOPE");
	int length;
	memset(address, 0, sizeof(*address));
	address->sun_family = AF_UNIX;
	length = snprintf(address->sun_path + 1, sizeof(address->sun_path) - 1,
	                  "worminal-workspace-%lu-%s", (unsigned long)getuid(), scope ? scope : "");
	if (length < 0 || length >= (int)sizeof(address->sun_path) - 1)
		return 0;
	return offsetof(struct sockaddr_un, sun_path) + 1 + length;
}

int
workspace_connect(void)
{
	struct sockaddr_un address;
	socklen_t length = workspace_address(&address);
	int fd;
	if (!length)
		return -1;
	fd = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
	if (fd < 0)
		return -1;
	if (connect(fd, (struct sockaddr *)&address, length) < 0) {
		close(fd);
		return -1;
	}
	return fd;
}

int
workspace_listen(void)
{
	struct sockaddr_un address;
	socklen_t length = workspace_address(&address);
	int fd;
	if (!length)
		return -1;
	fd = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
	if (fd < 0)
		return -1;
	if (bind(fd, (struct sockaddr *)&address, length) < 0 || listen(fd, 16) < 0) {
		close(fd);
		return -1;
	}
	return fd;
}

static int
workspace_spawn(void)
{
	char path[4096];
	ssize_t length = readlink("/proc/self/exe", path, sizeof(path) - 1);
	char *slash;
	pid_t child;
	if (length < 0 || length >= (ssize_t)sizeof(path) - 1)
		return 0;
	path[length] = '\0';
	slash = strrchr(path, '/');
	if (!slash || (size_t)(slash - path) + sizeof("/worminald") >= sizeof(path))
		return 0;
	strcpy(slash + 1, "worminald");
	child = fork();
	if (child < 0)
		return 0;
	if (child == 0) {
		int nullfd = open("/dev/null", O_RDWR);
		setsid();
		if (nullfd >= 0) {
			dup2(nullfd, 0);
			dup2(nullfd, 1);
			dup2(nullfd, 2);
			if (nullfd > 2)
				close(nullfd);
		}
		execl(path, path, "--serve", (char *)NULL);
		_exit(127);
	}
	return 1;
}

int
workspace_ensure(void)
{
	struct timespec pause = {.tv_nsec = 10000000};
	int fd = workspace_connect();
	if (fd >= 0)
		return fd;
	if (!workspace_spawn())
		return -1;
	for (int attempt = 0; attempt < 100; attempt++) {
		nanosleep(&pause, NULL);
		fd = workspace_connect();
		if (fd >= 0)
			return fd;
	}
	return -1;
}
