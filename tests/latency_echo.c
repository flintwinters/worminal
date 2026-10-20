/* Minimal PTY fixture: no interpreter, shell initialization, or timing hooks. */
#define _DEFAULT_SOURCE
#include <errno.h>
#include <signal.h>
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
main(void)
{
	struct termios mode;
	const char ready[] = "\033[?25l\033[48;2;17;34;51m\033[2J\033[H";
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
			if (initial && write(1, ready, sizeof(ready) - 1) != sizeof(ready) - 1) return 1;
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
		initial = 0;
		if (write(1, echo, sizeof(echo) - 1) < 0) return 1;
	}
}
