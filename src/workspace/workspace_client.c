#include <errno.h>
#include <signal.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <unistd.h>
#include "workspace_client.h"
#include "wire.h"
#include "workspace_socket.h"

int
workspace_client_open(const char *master)
{
	return workspace_client_open_process(master, NULL);
}

int
workspace_client_open_process(const char *master, pid_t *process)
{
	if (process) *process = 0;
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
		/* Full terminal frames are mostly repeated glyph fields; SSH compression
		 * keeps first visits to existing tabs responsive over the bridge. */
		execlp("ssh", "ssh", "-T", "-C", master, WORKSPACE_DAEMON, "--bridge", (char *)NULL);
		_exit(127);
	}
	close(sockets[1]);
	if (process) *process = child;
	return sockets[0];
}
