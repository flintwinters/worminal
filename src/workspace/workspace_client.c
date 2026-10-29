#include <errno.h>
#include <spawn.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <unistd.h>
#include "workspace_client.h"
#include "wire.h"
#include "workspace_socket.h"

extern char **environ;

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
	/* GTK calls this from a worker while the main thread constructs widgets.
	 * Spawn must not run application-wide fork hooks against concurrent GUI state. */
	posix_spawn_file_actions_t actions;
	int error = posix_spawn_file_actions_init(&actions);
	if (!error) {
		error = posix_spawn_file_actions_addclose(&actions, sockets[0]);
		if (!error) error = posix_spawn_file_actions_adddup2(&actions, sockets[1], 0);
		if (!error) error = posix_spawn_file_actions_adddup2(&actions, sockets[1], 1);
		if (!error && sockets[1] > 1) error = posix_spawn_file_actions_addclose(&actions, sockets[1]);
		/* Full frames contain repeated glyph fields; retain SSH compression. */
		char *args[] = {"ssh", "-T", "-C", (char *)master, WORKSPACE_DAEMON, "--bridge", NULL};
		if (!error) error = posix_spawnp(&child, "ssh", &actions, NULL, args, environ);
		posix_spawn_file_actions_destroy(&actions);
	}
	close(sockets[1]);
	if (error) {
		close(sockets[0]);
		errno = error;
		return -1;
	}
	if (process) *process = child;
	return sockets[0];
}
