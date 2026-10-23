/* Private failure injection and journal capture for daemon integration checks. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/select.h>
#include <sys/ioctl.h>
#include <syslog.h>
#include <termios.h>

static void
record_log(int priority, const char *format, va_list args)
{
	fprintf(stderr, "JOURNAL[%d] ", priority);
	vfprintf(stderr, format, args);
	fputc('\n', stderr);
}

void
syslog(int priority, const char *format, ...)
{
	va_list args;
	va_start(args, format);
	record_log(priority, format, args);
	va_end(args);
}

void
__syslog_chk(int priority, int flags, const char *format, ...)
{
	(void)flags;
	va_list args;
	va_start(args, format);
	record_log(priority, format, args);
	va_end(args);
}

int
pselect(int count, fd_set *readers, fd_set *writers, fd_set *errors,
        const struct timespec *timeout, const sigset_t *mask)
{
	if (getenv("WORMINAL_TEST_PSELECT_FAILURE")) { errno = EBADF; return -1; }
	int (*original)(int, fd_set *, fd_set *, fd_set *, const struct timespec *, const sigset_t *) =
		dlsym(RTLD_NEXT, "pselect");
	return original(count, readers, writers, errors, timeout, mask);
}

int
openpty(int *master, int *slave, char *name, const struct termios *term, const struct winsize *size)
{
	(void)master; (void)slave; (void)name; (void)term; (void)size;
	errno = EMFILE;
	return -1;
}
