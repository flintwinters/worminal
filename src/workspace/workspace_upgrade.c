#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <poll.h>
#include <signal.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/inotify.h>
#include <sys/socket.h>
#include <sys/select.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <syslog.h>
#include <unistd.h>
#include "workspace_upgrade.h"
#include "workspace_handoff.h"

static char executable[PATH_MAX], filename[NAME_MAX + 1];
static struct stat loaded, attempted;
static int first_check;

int
workspace_upgrade_watch(void)
{
	/* Only the release workspace upgrades automatically. v1 remains an explicit rollback. */
#ifndef WORMINAL_PREVIEW_PROTOCOL
	return -1;
#else
	char directory[PATH_MAX];
	ssize_t size = readlink("/proc/self/exe", executable, sizeof(executable) - 1);
	if (size < 0 || size >= (ssize_t)sizeof(executable) - 1 || stat("/proc/self/exe", &loaded) < 0) return -1;
	executable[size] = 0;
	char *deleted = strstr(executable, " (deleted)");
	if (deleted && !deleted[10]) *deleted = 0;
	strcpy(directory, executable);
	char *slash = strrchr(directory, '/');
	if (!slash || strlen(slash + 1) > NAME_MAX) return -1;
	strcpy(filename, slash + 1); *slash = 0;
	int fd = inotify_init1(IN_NONBLOCK | IN_CLOEXEC);
	if (fd < 0) return -1;
	if (fd >= FD_SETSIZE) { close(fd); return -1; }
	if (inotify_add_watch(fd, directory, IN_MOVED_TO | IN_CLOSE_WRITE | IN_ATTRIB) < 0) {
		close(fd); return -1;
	}
	first_check = 1;
	return fd;
#endif
}

static int
spawn_receiver(const char *path, int listener, int receiving, pid_t *child)
{
	extern char **environ;
	int copies[2] = {-1, -1}, error;
	posix_spawn_file_actions_t actions;
	error = posix_spawn_file_actions_init(&actions);
	if (error) return error;
	int sources[] = {listener, receiving};
	for (int i = 0; i < 2 && !error; i++) {
		copies[i] = fcntl(sources[i], F_DUPFD_CLOEXEC, 64);
		if (copies[i] < 0) error = errno;
		else error = posix_spawn_file_actions_adddup2(&actions, copies[i], 3 + i);
	}
	char *args[] = {(char *)path, "--upgrade-receiver", NULL};
	if (!error) error = posix_spawn(child, path, &actions, NULL, args, environ);
	posix_spawn_file_actions_destroy(&actions);
	for (int i = 0; i < 2; i++) if (copies[i] >= 0) close(copies[i]);
	return error;
}

int
workspace_upgrade_check(int watch, int listener, Tab *tabs, void (*remove_tab)(Tab *), uint32_t next_id)
{
	union { struct inotify_event alignment; char bytes[8192]; } events;
	int changed = first_check;
	first_check = 0;
	if (watch < 0) return 0;
	for (;;) {
		ssize_t size = read(watch, events.bytes, sizeof(events.bytes));
		if (size < 0 && errno == EINTR) continue;
		if (size <= 0) break;
		for (size_t pos = 0; pos < (size_t)size;) {
			struct inotify_event *event = (void *)(events.bytes + pos);
			if ((event->mask & IN_Q_OVERFLOW) || (event->len && !strcmp(event->name, filename))) changed = 1;
			pos += sizeof(*event) + event->len;
		}
	}
	struct stat replacement;
	if (!changed || stat(executable, &replacement) < 0 ||
	    (replacement.st_dev == loaded.st_dev && replacement.st_ino == loaded.st_ino) ||
	    (replacement.st_dev == attempted.st_dev && replacement.st_ino == attempted.st_ino)) return 0;
	attempted = replacement;
	if (!S_ISREG(replacement.st_mode) || access(executable, X_OK) < 0) return 0;
	char token[60];
	snprintf(token, sizeof(token), "upgrade-%ld-%lu", (long)getpid(), (unsigned long)replacement.st_ino);
	int receiving = handin_listen(token);
	pid_t child = -1;
	if (receiving < 0) return 0;
	int error = spawn_receiver(executable, listener, receiving, &child);
	int okay = !error && handoff_upgrade_send(token, tabs, remove_tab, next_id);
	close(receiving);
	if (okay) {
		syslog(LOG_NOTICE, "Daemon upgrade committed to pid %ld", (long)child);
		return 1;
	}
	if (child > 0) {
		/* No C was delivered: the child owns no PTYs. An unsupported binary
		 * may ignore SIGTERM; bounded rollback must not wait on its cooperation. */
		kill(child, SIGKILL);
		while (waitpid(child, NULL, 0) < 0 && errno == EINTR) {}
	}
	syslog(LOG_ERR, "Daemon upgrade rejected; live tabs retained%s%s",
	       error ? ": " : "", error ? strerror(error) : "");
	return 0;
}

int
workspace_upgrade_receive(Tab **tabs, Tab **last, uint32_t *next_id, int legacy)
{
	/* Ownership must outlive the launcher, including its process-group cleanup.
	 * Enforce isolation here, before either handoff variant can commit PTYs;
	 * redirecting stdio alone does not detach a daemon from its coordinator. */
	if (getsid(0) != getpid() && setsid() < 0) return 0;
	/* Only these two descriptors cross exec; shell children must never retain them.
	 * The shared listening socket keeps the workspace address bound throughout commit. */
	for (int fd = 3; fd <= 4; fd++) {
		int listening = 0;
		socklen_t size = sizeof(listening);
		if (getsockopt(fd, SOL_SOCKET, SO_ACCEPTCONN, &listening, &size) < 0 || !listening ||
		    fcntl(fd, F_SETFD, FD_CLOEXEC) < 0) return 0;
	}
	/* A replacement must not hold the workspace endpoint forever if its donor
	 * disappears before connecting. Pre-commit failure always leaves the donor in charge. */
	struct pollfd ready = {.fd = 4, .events = POLLIN};
	int status;
	do status = poll(&ready, 1, HANDOFF_TIMEOUT_SECONDS * 1000); while (status < 0 && errno == EINTR);
	if (status <= 0) { close(4); return 0; }
	uint32_t manifest[HANDOFF_MAX_TABS + 3];
	int okay;
	if (legacy) {
		/* Bootstrap older handoff-capable daemons without changing visible tab IDs.
		 * Validate the complete mapping before the sender can commit ownership. */
		struct stat st;
		if (fstat(5, &st) < 0 || !S_ISREG(st.st_mode) || st.st_size < 12 ||
		    st.st_size > (off_t)sizeof(manifest) ||
		    pread(5, manifest, st.st_size, 0) != st.st_size ||
		    manifest[0] > HANDOFF_MAX_TABS || manifest[2] > INT_MAX ||
		    st.st_size != (off_t)((manifest[0] + 3) * sizeof(uint32_t))) {
			close(5); close(4); return 0;
		}
		close(5);
		okay = handin_mapped_accept(4, tabs, last, next_id, manifest + 3, manifest[0], manifest[1], manifest[2]);
	} else okay = handin_upgrade_accept(4, tabs, last, next_id);
	close(4);
	return okay;
}
