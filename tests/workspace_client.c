/* SSH startup must bypass GUI fork hooks and preserve its duplex stdio transport. */
#define _GNU_SOURCE
#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>
#include "../src/workspace/workspace_client.h"

static int fork_hooks;
static void before_fork(void) { fork_hooks++; }
int workspace_ensure(void) { return 42; }

static void
check_transport(void)
{
	pid_t child = 0;
	int fd = workspace_client_open_process("spawn-test", &child);
	assert(fd >= 0 && child > 0);
	assert(fork_hooks == 0);
	assert(fcntl(fd, F_GETFD) & FD_CLOEXEC);
	assert(write(fd, "x", 1) == 1);
	char reply;
	assert(read(fd, &reply, 1) == 1 && reply == 'x');
	close(fd);
	int status;
	assert(waitpid(child, &status, 0) == child);
	assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
}

int
main(int argc, char **argv)
{
	if (argc > 1) {
		assert(argc == 6 && !strcmp(argv[1], "-T") && !strcmp(argv[2], "-C"));
		assert(!strcmp(argv[3], "spawn-test") && !strcmp(argv[4], "worminald"));
		assert(!strcmp(argv[5], "--bridge"));
		char byte;
		assert(read(0, &byte, 1) == 1);
		assert(write(1, &byte, 1) == 1);
		return 0;
	}
	assert(pthread_atfork(before_fork, NULL, NULL) == 0);
	assert(mkdir(".checks", 0700) == 0 || errno == EEXIST);
	char directory[] = ".checks/spawn-client-XXXXXX", path[256];
	assert(mkdtemp(directory));
	assert(snprintf(path, sizeof(path), "%s/ssh", directory) > 0);
	assert(symlink("/proc/self/exe", path) == 0);
	assert(setenv("PATH", directory, 1) == 0);
	check_transport();
	/* Closed launch stdio may place either socket endpoint at fd 0 or 1. */
	int input = fcntl(0, F_DUPFD_CLOEXEC, 3), output = fcntl(1, F_DUPFD_CLOEXEC, 3);
	assert(input >= 0 && output >= 0);
	close(0); close(1);
	check_transport();
	assert(dup2(input, 0) == 0 && dup2(output, 1) == 1);
	close(input); close(output);
	assert(unlink(path) == 0);
	pid_t child = -1;
	assert(workspace_client_open_process("spawn-test", &child) == -1 && errno == ENOENT && child == 0);
	assert(workspace_client_open_process("-invalid", &child) == -1 && errno == EINVAL && child == 0);
	assert(workspace_client_open_process(NULL, &child) == 42 && child == 0);
	assert(rmdir(directory) == 0);
	return 0;
}
