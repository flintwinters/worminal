/* Session children: launch context, per-session reaping, hangup, stop, and alternate-screen permission. */
#include <poll.h>

static TermSession *
spawn_session(const char *script)
{
	char *args[] = {"/bin/sh", "-c", (char *)script, NULL};
	TermSession *created = tsessionnew(8, 2);
	ttynew(NULL, NULL, NULL, args);
	assert(created->child_pid > 0 && tsessionpid(created) == created->child_pid);
	return created;
}

static void
wait_reaped(TermSession *chosen)
{
	for (int i = 0; i < 500 && chosen->child_pid; i++) {
		tsessionreap();
		if (chosen->child_pid)
			usleep(10000);
	}
	assert(chosen->child_pid == 0 && tsessionpid(chosen) == 0);
}

static size_t
read_until_hangup(int fd, char *buffer, size_t size)
{
	size_t used = 0;
	struct pollfd ready = {.fd = fd, .events = POLLIN};
	while (used < size - 1 && poll(&ready, 1, 5000) > 0) {
		ssize_t got = read(fd, buffer + used, size - 1 - used);
		if (got <= 0)
			break;
		used += got;
	}
	buffer[used] = '\0';
	return used;
}

static void
check_session_launch(void)
{
	char directory[] = "build/session-launch-XXXXXX", resolved[PATH_MAX], expected[PATH_MAX + 64];
	char printer[sizeof(directory) + 16], output[PATH_MAX + 64];
	/* applylaunch splits entries in place, as forwarded wire strings allow. */
	char entry[] = "LAUNCH_ONLY=kept", *environment[] = {entry, NULL};
	assert(mkdtemp(directory) && realpath(directory, resolved));
	setenv("OWNER_ONLY", "leaked", 1);
	/* The child adopts the forwarded cwd and environment, never the owner's. */
	ttysetlaunch(directory, environment, "42");
	char *args[] = {"/bin/sh", "-c", "printf '%s|%s|%s|%s' \"$(pwd -P)\" \"$LAUNCH_ONLY\" "
	                "\"${OWNER_ONLY-unset}\" \"$WINDOWID\"", NULL};
	TermSession *launched = tsessionnew(8, 2);
	/* A relative printer path resolves against the launch cwd, not the owner's. */
	ttynew(NULL, NULL, "printer", args);
	ttysetlaunch(NULL, NULL, NULL);
	snprintf(expected, sizeof(expected), "%s|kept|unset|42", resolved);
	read_until_hangup(launched->pty_fd, output, sizeof(output));
	assert(strcmp(output, expected) == 0);
	snprintf(printer, sizeof(printer), "%s/printer", directory);
	assert(launched->output_fd > 1 && access(printer, F_OK) == 0 && access("printer", F_OK) != 0);
	assert(getenv("OWNER_ONLY") && !getenv("LAUNCH_ONLY"));
	wait_reaped(launched);
	tsessionremove(launched);
	unsetenv("OWNER_ONLY");
	assert(unlink(printer) == 0 && rmdir(directory) == 0);
}

static void
check_session_process(void)
{
	TermSession *previous = tsessioncurrent();
	check_session_launch();

	/* Reaping one child records its status without touching other sessions. */
	TermSession *exiting = spawn_session("exit 3"), *waiting = spawn_session("exec sleep 30");
	TermSession *stopped = spawn_session("exec sleep 30");
	wait_reaped(exiting);
	assert(tsessionexitstatus() == 3);
	assert(tsessionpid(waiting) == waiting->child_pid && waiting->child_pid > 0);

	/* Stopping closes the PTY and hangs up that child only. */
	int fd = tsessionfd(stopped);
	tsessionstop(stopped);
	assert(tsessionfd(stopped) == -1 && fcntl(fd, F_GETFD) == -1 && errno == EBADF);
	wait_reaped(stopped);
	assert(tsessionexitstatus() == 1 && waiting->child_pid > 0);

	/* Hanging up every session restores the selected one. */
	tsessionuse(exiting);
	tsessionhangupall();
	assert(tsessioncurrent() == exiting);
	wait_reaped(waiting);
	assert(tsessionexitstatus() == 1);
	tsessionremove(exiting);
	tsessionremove(waiting);
	tsessionremove(stopped);

	/* A session without alternate-screen permission keeps drawing on the main screen. */
	TermSession *plain = tsessionnew(4, 2);
	const char *enter = "\033[?1049h\033[?47h\033[?1047h";
	tsessionallowalt(plain, 0);
	twrite(enter, strlen(enter), 0);
	assert(!tisaltscr());
	tsessionallowalt(plain, 1);
	twrite("\033[?1049h", 8, 0);
	assert(tisaltscr());
	twrite("\033[?1049l", 8, 0);
	assert(!tisaltscr());
	tsessionremove(plain);
	tsessionuse(previous);
}
