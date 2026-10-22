/* Minimal PTY fixture: no interpreter, shell initialization, or timing hooks. */
#define _DEFAULT_SOURCE
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/select.h>
#include <termios.h>
#include <unistd.h>

static volatile sig_atomic_t resized = 1;

static void
resize(int signal)
{
	(void)signal;
	resized = 1;
}

int
main(int argc, char **argv)
{
	struct termios mode;
	unsigned long color = 0x112233;
	int history = 0;
	char **command = NULL;
	for (int i = 1; i < argc; i++) {
		if (!strcmp(argv[i], "--history")) history = 1;
		else if (!strcmp(argv[i], "--exec") && i + 1 < argc) { command = argv + i + 1; break; }
		else if (!strcmp(argv[i], "--color") && ++i < argc) {
			char *end;
			color = strtoul(argv[i], &end, 16);
			if (!*argv[i] || *end || color > 0xffffff) return 2;
		} else return 2;
	}
	char ready[80];
	int ready_length = snprintf(ready, sizeof(ready), "\033[?25l\033[48;2;%lu;%lu;%lum\033[2J\033[H",
	    color >> 16, (color >> 8) & 255, color & 255);
	char echo[] = "\033[12;40H ";
	int initial = 1;
	struct sigaction action = {.sa_handler = resize};
	sigset_t blocked, previous;
	sigemptyset(&action.sa_mask);
	if (sigaction(SIGWINCH, &action, NULL) < 0) return 1;
	sigemptyset(&blocked);
	sigaddset(&blocked, SIGWINCH);
	if (sigprocmask(SIG_BLOCK, &blocked, &previous) < 0) return 1;
	if (tcgetattr(0, &mode) < 0) return 1;
	cfmakeraw(&mode);
	if (tcsetattr(0, TCSANOW, &mode) < 0) return 1;
	for (;;) {
		if (resized) {
			resized = 0;
			/* Early GTK allocations can resize the PTY after its first output.
			 * Restore the ready canvas until the first measured key arrives. */
			if (initial) {
				if (history) {
					const char background[] = "\033[48;2;68;85;102m";
					if (write(1, background, sizeof(background) - 1) != sizeof(background) - 1) return 1;
					for (int row = 0; row < 256; row++)
						if (write(1, "history\r\n", 9) != 9) return 1;
				}
				if (write(1, ready, ready_length) != ready_length) return 1;
			}
		}
		fd_set readers;
		FD_ZERO(&readers);
		FD_SET(0, &readers);
		if (pselect(1, &readers, NULL, NULL, NULL, &previous) < 0) {
			if (errno == EINTR) continue;
			return 1;
		}
		ssize_t count = read(0, echo + sizeof(echo) - 2, 1);
		if (count < 0 && errno == EINTR) continue;
		if (count != 1) return count < 0;
		if (command) { execvp(command[0], command); perror(command[0]); return 1; }
		initial = 0;
		if (write(1, echo, sizeof(echo) - 1) < 0) return 1;
	}
}
