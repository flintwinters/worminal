/* See LICENSE for license details. */
#include <ctype.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <pwd.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <signal.h>
#include <sys/ioctl.h>
#include <sys/select.h>
#include <syslog.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <termios.h>
#include <unistd.h>
#include <wchar.h>

#include "st.h"
#include "win.h"
#include "st_state.h"
#include "session_transfer.h"
#include "render_sync.h"
#include WORMINAL_THEME_HEADER

#if   defined(__linux)
 #include <pty.h>
#elif defined(__OpenBSD__) || defined(__NetBSD__) || defined(__APPLE__)
 #include <util.h>
#elif defined(__FreeBSD__) || defined(__DragonFly__)
 #include <libutil.h>
#endif

static void execsh(char *, char **);
static void stty(char **);
static void sigchld(int);


void tlegacyprint(char *, size_t);

static void drawregion(int, int, int, int);

static ssize_t xwrite(int, const char *, size_t);

void *
xmalloc(size_t len)
{
	void *p;

	if (!(p = malloc(len)))
		die("malloc: %s\n", strerror(errno));

	return p;
}

void *
xrealloc(void *p, size_t len)
{
	if ((p = realloc(p, len)) == NULL)
		die("realloc: %s\n", strerror(errno));

	return p;
}

char *
xstrdup(const char *s)
{
	char *p;

	if ((p = strdup(s)) == NULL)
		die("strdup: %s\n", strerror(errno));

	return p;
}

static TermSession primarysession = {.output_fd = 1, .pty_fd = -1,
                                     .engine.altscreen_allowed = 1};
static TermSession *session = &primarysession;
static TermSession *sessions = &primarysession;
static int last_child_status;
/* The service parses OSC 52; keep this policy with the shared parser. */
int allowwindowops = 1;
static const char *launch_cwd, *launch_windowid;
static char **launch_env;
void (*ttyrawhook)(const char *, size_t);
void (*ttywritehook)(const char *, size_t, int);
void (*tscrollhook)(int, int);
void (*tprinterhook)(const char *, size_t);
#define term (session->engine.term)
#define sel (session->engine.sel)
#define csiescseq (session->engine.csiescseq)
#define strescseq (session->engine.strescseq)
#define iofd (session->output_fd)
#define cmdfd (session->pty_fd)
#define pid (session->child_pid)

#include "engine_legacy.h"

TermSession *
tsessioncurrent(void)
{
	return session;
}

TermSession *
tsessionalloc(void)
{
	TermSession *created = xmalloc(sizeof(*created));
	TermSession **tail = &sessions;
	memset(created, 0, sizeof(*created));
	created->output_fd = 1;
	created->pty_fd = -1;
	created->engine.altscreen_allowed = 1;
	while (*tail)
		tail = &(*tail)->next;
	*tail = created;
	session = created;
	return created;
}

TermSession *
tsessionnew(int cols, int rows)
{
	TermSession *created = tsessionalloc();
	tnew(cols, rows);
	selinit();
	return created;
}

TermSession *
tsessionnext(TermSession *current)
{
	return current ? current->next : sessions;
}

void
tsessionuse(TermSession *chosen)
{
	if (!chosen)
		die("missing terminal session\n");
	session = chosen;
}

int
tsessionfd(TermSession *chosen)
{
	return chosen->pty_fd;
}

pid_t
tsessionpid(TermSession *chosen)
{
	return tsessionlivepid(chosen);
}

void
tsessionallowalt(TermSession *chosen, int allowed)
{
	chosen->engine.altscreen_allowed = allowed;
}

void
tsessionreap(void)
{
	TermSession *candidate;
	int status;
	pid_t child;

	while ((child = waitpid(-1, &status, WNOHANG)) > 0) {
		for (candidate = sessions; candidate; candidate = candidate->next)
			if (candidate->child_pid == child) {
				candidate->child_pid = 0;
				last_child_status = WIFEXITED(status) ? WEXITSTATUS(status) : 1;
				break;
			}
	}
}

int
tsessionexitstatus(void)
{
	return last_child_status;
}

void
tsessionhangupall(void)
{
	TermSession *previous = session, *candidate;

	for (candidate = sessions; candidate; candidate = candidate->next) {
		session = candidate;
		ttyhangup();
	}
	session = previous;
}

void
tsessionstop(TermSession *chosen)
{
	tsessionstopchild(chosen);
	if (chosen->pty_fd >= 0) {
		close(chosen->pty_fd);
		chosen->pty_fd = -1;
	}
}

#undef term
#undef strescseq
void
tsessionremove(TermSession *chosen)
{
	TermSession **slot = &sessions;

	while (*slot && *slot != chosen)
		slot = &(*slot)->next;
	if (!*slot)
		die("removing an unknown terminal session\n");
	tsessionstop(chosen);
	if (chosen->output_fd > 1)
		close(chosen->output_fd);
	*slot = chosen->next;
	tsessionfreegrid(chosen);
	free(chosen->engine.strescseq.buf);
	te_freeevents(&chosen->engine);
	if (session == chosen)
		session = sessions;
	if (chosen != &primarysession)
		free(chosen);
}
#define term (session->engine.term)
#define strescseq (session->engine.strescseq)

void
ttysetlaunch(const char *cwd, char **env, const char *windowid)
{
	launch_cwd = cwd;
	launch_env = env;
	launch_windowid = windowid;
}

static void
applylaunch(void)
{
	char **entry;
	extern char **environ;
	if (launch_cwd && chdir(launch_cwd) < 0)
		_exit(1);
	if (launch_env) {
		environ = NULL;
		for (entry = launch_env; *entry; entry++) {
			char *equal = strchr(*entry, '=');
			if (equal) {
				*equal = '\0';
				setenv(*entry, equal + 1, 1);
				*equal = '=';
			}
		}
	}
	if (launch_windowid)
		setenv("WINDOWID", launch_windowid, 1);
}

static int
launchopen(const char *path, int flags, mode_t mode)
{
	char *full;
	int fd;
	if (!launch_cwd || path[0] == '/')
		return open(path, flags, mode);
	full = xmalloc(strlen(launch_cwd) + strlen(path) + 2);
	sprintf(full, "%s/%s", launch_cwd, path);
	fd = open(full, flags, mode);
	free(full);
	return fd;
}

ssize_t
xwrite(int fd, const char *s, size_t len)
{
	size_t aux = len;
	ssize_t r;

	while (len > 0) {
		r = write(fd, s, len);
		if (r < 0)
			return r;
		len -= r;
		s += r;
	}

	return aux;
}

void
die(const char *errstr, ...)
{
	va_list ap;
	char message[1024];

	va_start(ap, errstr);
	vsnprintf(message, sizeof(message), errstr, ap);
	va_end(ap);
	syslog(LOG_ERR, "%s", message);
	va_start(ap, errstr);
	vfprintf(stderr, errstr, ap);
	va_end(ap);
	exit(1);
}

void
execsh(char *cmd, char **args)
{
	char *sh, *prog, *arg;
	const struct passwd *pw;

	errno = 0;
	if ((pw = getpwuid(getuid())) == NULL) {
		if (errno)
			die("getpwuid: %s\n", strerror(errno));
		else
			die("who are you?\n");
	}

	if ((sh = getenv("SHELL")) == NULL)
		sh = (pw->pw_shell[0]) ? pw->pw_shell : cmd;

	if (args) {
		prog = args[0];
		arg = NULL;
	} else if (scroll) {
		prog = scroll;
		arg = utmp ? utmp : sh;
	} else if (utmp) {
		prog = utmp;
		arg = NULL;
	} else {
		prog = sh;
		arg = NULL;
	}
	DEFAULT(args, ((char *[]) {prog, arg, NULL}));

	unsetenv("COLUMNS");
	unsetenv("LINES");
	unsetenv("TERMCAP");
	setenv("LOGNAME", pw->pw_name, 1);
	setenv("USER", pw->pw_name, 1);
	setenv("SHELL", sh, 1);
	setenv("HOME", pw->pw_dir, 1);
	setenv("TERM", termname, 1);

	signal(SIGCHLD, SIG_DFL);
	signal(SIGHUP, SIG_DFL);
	signal(SIGINT, SIG_DFL);
	signal(SIGQUIT, SIG_DFL);
	signal(SIGTERM, SIG_DFL);
	signal(SIGALRM, SIG_DFL);

	execvp(prog, args);
	_exit(1);
}

void
sigchld(int a)
{
	/* Interrupt pselect; the owner loop reaps every session child. */
	(void)a;
}

void
stty(char **args)
{
	char cmd[_POSIX_ARG_MAX], **p, *q, *s;
	size_t n, siz;

	if ((n = strlen(stty_args)) > sizeof(cmd)-1)
		die("incorrect stty parameters\n");
	memcpy(cmd, stty_args, n);
	q = cmd + n;
	siz = sizeof(cmd) - n;
	for (p = args; p && (s = *p); ++p) {
		if ((n = strlen(s)) > siz-1)
			die("stty parameter length too long\n");
		*q++ = ' ';
		memcpy(q, s, n);
		q += n;
		siz -= n + 1;
	}
	*q = '\0';
	if (system(cmd) != 0)
		perror("Couldn't call stty");
}

int
ttynew(const char *line, char *cmd, const char *out, char **args)
{
	int m, s;
	struct winsize w = { .ws_row = term.row, .ws_col = term.col };

	if (out) {
		term.mode |= MODE_PRINT;
		iofd = (!strcmp(out, "-")) ?
			  1 : launchopen(out, O_WRONLY | O_CREAT, 0666);
		if (iofd < 0) {
			fprintf(stderr, "Error opening %s:%s\n",
				out, strerror(errno));
		}
	}

	if (line) {
		int status;
		pid_t configurator;
		if ((cmdfd = launchopen(line, O_RDWR, 0)) < 0)
			die("open line '%s' failed: %s\n",
			    line, strerror(errno));
		/* Configure the line in a child so a forwarded -l never replaces
		 * the owner's stdin or changes its process environment. */
		configurator = fork();
		if (configurator < 0)
			die("fork for stty failed: %s\n", strerror(errno));
		if (configurator == 0) {
			dup2(cmdfd, 0);
			applylaunch();
			stty(args);
			_exit(0);
		}
		while (waitpid(configurator, &status, 0) < 0)
			if (errno != EINTR)
				die("wait for stty failed: %s\n", strerror(errno));
		return cmdfd;
	}

	/* seems to work fine on linux, openbsd and freebsd */
	/* The child may exec before the parent reaches its first resize. */
	if (openpty(&m, &s, NULL, NULL, &w) < 0)
		die("openpty failed: %s\n", strerror(errno));

	switch (pid = fork()) {
	case -1:
		die("fork failed: %s\n", strerror(errno));
		break;
	case 0:
		close(iofd);
		close(m);
		applylaunch();
		setsid(); /* create a new process group */
		dup2(s, 0);
		dup2(s, 1);
		dup2(s, 2);
		if (ioctl(s, TIOCSCTTY, NULL) < 0)
			die("ioctl TIOCSCTTY failed: %s\n", strerror(errno));
		if (s > 2)
			close(s);
#ifdef __OpenBSD__
		if (pledge("stdio getpw proc exec", NULL) == -1)
			die("pledge\n");
#endif
		execsh(cmd, args);
		break;
	default:
#ifdef __OpenBSD__
		if (pledge("stdio rpath tty proc", NULL) == -1)
			die("pledge\n");
#endif
		close(s);
		cmdfd = m;
		signal(SIGCHLD, sigchld);
		break;
	}
	return cmdfd;
}

size_t
ttyread(void)
{
	char *buf = session->ttybuf;
	int buflen = session->ttybuflen;
	int ret, written;

	/* append read bytes to unprocessed bytes */
	ret = read(cmdfd, buf+buflen, sizeof(session->ttybuf)-buflen);

	switch (ret) {
	case 0:
		close(cmdfd);
		cmdfd = -1;
		return 0;
	case -1:
		if (errno == EINTR || errno == EAGAIN || errno == EWOULDBLOCK)
			return 0;
		if (errno == EIO) {
			close(cmdfd);
			cmdfd = -1;
			return 0;
		}
		die("couldn't read from shell: %s\n", strerror(errno));
	default:
		buflen += ret;
		written = twrite(buf, buflen, 0);
		buflen -= written;
		/* keep any incomplete UTF-8 byte sequence for the next call */
		if (buflen > 0)
			memmove(buf, buf + written, buflen);
		session->ttybuflen = buflen;
		return ret;
	}
}

void
ttywrite(const char *s, size_t n, int may_echo)
{
    if (ttywritehook) {
        ttywritehook(s, n, may_echo);
        return;
    }
    if (cmdfd < 0) return;
    tlegacyinput(s, n, may_echo);
}

void
tlegacyraw(const char *s, size_t n)
{
	if (ttyrawhook) {
		ttyrawhook(s, n);
		return;
	}
	fd_set wfd, rfd;
	ssize_t r;
	size_t lim = 256;

	/* Legacy local writes drain PTY output while waiting for input capacity. */
	while (n > 0) {
		FD_ZERO(&wfd);
		FD_ZERO(&rfd);
		FD_SET(cmdfd, &wfd);
		FD_SET(cmdfd, &rfd);

		/* Check if we can write. */
		if (pselect(cmdfd+1, &rfd, &wfd, NULL, NULL, NULL) < 0) {
			if (errno == EINTR)
				continue;
			die("select failed: %s\n", strerror(errno));
		}
		if (FD_ISSET(cmdfd, &wfd)) {
			/*
			 * Only write the bytes written by ttywrite() or the
			 * default of 256. This seems to be a reasonable value
			 * for a serial line. Bigger values might clog the I/O.
			 */
			if ((r = write(cmdfd, s, (n < lim)? n : lim)) <= 0) {
				if (r < 0 && errno == EINTR)
					continue;
				if (r == 0 || errno == EIO || errno == EBADF) {
					tsessionstop(session);
					return;
				}
				goto write_error;
			}
			if (r < n) {
				/*
				 * We weren't able to write out everything.
				 * This means the buffer is getting full
				 * again. Empty it.
				 */
				if (n < lim)
					lim = ttyread();
				if (cmdfd < 0)
					return;
				n -= r;
				s += r;
			} else {
				/* All bytes have been written. */
				break;
			}
		}
		if (FD_ISSET(cmdfd, &rfd))
			lim = ttyread();
		if (cmdfd < 0)
			return;
	}
	return;

write_error:
	die("write error on tty: %s\n", strerror(errno));
}

void
ttyresize(int tw, int th)
{
	struct winsize w;

	if (cmdfd < 0)
		return;

	w.ws_row = term.row;
	w.ws_col = term.col;
	w.ws_xpixel = tw;
	w.ws_ypixel = th;
	if (ioctl(cmdfd, TIOCSWINSZ, &w) < 0)
		fprintf(stderr, "Couldn't set window size: %s\n", strerror(errno));
}

void
ttyhangup(void)
{
	/* Send SIGHUP to shell */
	tsessionsignal(session, SIGHUP);
}

/* The viewport uses history only for rows above the live screen. */

/* Read the shared screen through a view's independent scroll offset. */

/* Store a row without copying glyphs; reuse the evicted row as scratch space. */

/* for absolute user moves, when decom is set */

void
sendbreak(const Arg *arg)
{
	if (tcsendbreak(cmdfd, 0))
		perror("Error sending break");
}

void
tlegacyprint(char *s, size_t len)
{
	if (iofd == 1 && tprinterhook) {
		tprinterhook(s, len);
		return;
	}
	if (iofd != -1 && xwrite(iofd, s, len) < 0) {
		perror("Error writing to output file");
		close(iofd);
		iofd = -1;
	}
}

/*
 * returns 1 when the sequence is finished and it hasn't to read
 * more characters for this sequence, otherwise 0
 */

void
drawregion(int x1, int y1, int x2, int y2)
{
	int y;
	int pass;

	if (xoverlap()) {
		/* Backgrounds erase overhangs; repaint every row to remove old spills. */
		for (pass = DRAW_BACKGROUND; pass <= DRAW_FOREGROUND; pass <<= 1)
			for (y = y1; y < y2; y++)
				xdrawline(tline(y), x1, y, x2, pass);
		for (y = y1; y < y2; y++)
			term.dirty[y] = 0;
		return;
	}

	for (y = y1; y < y2; y++) {
		if (!term.dirty[y])
			continue;

		term.dirty[y] = 0;
		xdrawline(tline(y), x1, y, x2,
			DRAW_BACKGROUND | DRAW_FOREGROUND);
	}
}

void
draw(void)
{
	int cx = term.c.x, ocx = term.ocx, ocy = term.ocy;

	if (!xstartdraw())
		return;

	/* adjust cursor position */
	LIMIT(term.ocx, 0, term.col-1);
	LIMIT(term.ocy, 0, term.row-1);
	if (term.line[term.ocy][term.ocx].mode & ATTR_WDUMMY)
		term.ocx--;
	if (term.line[term.c.y][cx].mode & ATTR_WDUMMY)
		cx--;

	drawregion(0, 0, term.col, term.row);
	if (!term.scr && !session->remote_scroll)
		xdrawcursor(cx, term.c.y, term.line[term.c.y][cx],
				term.ocx, term.ocy, term.line[term.ocy][term.ocx]);
	term.ocx = cx;
	term.ocy = term.c.y;
	xfinishdraw();
	if (ocx != term.ocx || ocy != term.ocy)
		xximspot(term.ocx, term.ocy);
}

void
redraw(void)
{
	tfulldirt();
	draw();
}
