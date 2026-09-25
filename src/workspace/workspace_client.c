#include <errno.h>
#include <signal.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <unistd.h>
#include "workspace_client.h"
#include "workspace_socket.h"

int
workspace_client_open(const char *master)
{
	int sockets[2];
	pid_t child;
	if (!master)
		return workspace_ensure();
	if (!*master || *master == '-' || strchr(master, '\n')) {
		errno = EINVAL;
		return -1;
	}
	if (socketpair(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0, sockets) < 0)
		return -1;
	child = fork();
	if (child < 0) {
		close(sockets[0]);
		close(sockets[1]);
		return -1;
	}
	if (child == 0) {
		dup2(sockets[1], 0);
		dup2(sockets[1], 1);
		close(sockets[0]);
		close(sockets[1]);
		execlp("ssh", "ssh", "-T", master, "worminald", "--bridge", (char *)NULL);
		_exit(127);
	}
	close(sockets[1]);
	return sockets[0];
}
