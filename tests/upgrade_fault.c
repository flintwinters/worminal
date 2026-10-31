/* Private receiver-setup failure injection and journal capture. No application code loads this. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/signalfd.h>
#include <sys/socket.h>
#include <unistd.h>

static int
receiver_fault(const char *operation)
{
	const char *path = getenv("WORMINAL_TEST_UPGRADE_FAULT_FILE");
	const char *expected = getenv("WORMINAL_TEST_UPGRADE_FAULT_OPERATION");
	if (!path || !expected || strcmp(operation, expected) || access(path, F_OK)) return 0;
	int fd = open("/proc/self/cmdline", O_RDONLY | O_CLOEXEC);
	if (fd < 0) return 0;
	char bytes[513] = {0};
	ssize_t count = read(fd, bytes, sizeof(bytes) - 1);
	close(fd);
	for (ssize_t position = 0; position < count; position += strlen(bytes + position) + 1)
		if (!strcmp(bytes + position, "--upgrade-receiver")) return 1;
	return 0;
}

int
signalfd(int fd, const sigset_t *mask, int flags)
{
	if (receiver_fault("signals")) { errno = EMFILE; return -1; }
	int (*original)(int, const sigset_t *, int) = dlsym(RTLD_NEXT, "signalfd");
	return original(fd, mask, flags);
}

int
setsockopt(int fd, int level, int option, const void *value, socklen_t length)
{
	if (receiver_fault("transport") && level == SOL_SOCKET && option == SO_RCVTIMEO) {
		errno = EMFILE; return -1;
	}
	int (*original)(int, int, int, const void *, socklen_t) = dlsym(RTLD_NEXT, "setsockopt");
	return original(fd, level, option, value, length);
}

static void
record(int priority, const char *format, va_list args)
{
	fprintf(stderr, "UPGRADE-JOURNAL[%d] ", priority);
	vfprintf(stderr, format, args);
	fputc('\n', stderr);
}

void
syslog(int priority, const char *format, ...)
{
	va_list args;
	va_start(args, format);
	record(priority, format, args);
	va_end(args);
}

void
__syslog_chk(int priority, int flags, const char *format, ...)
{
	(void)flags;
	va_list args;
	va_start(args, format);
	record(priority, format, args);
	va_end(args);
}
