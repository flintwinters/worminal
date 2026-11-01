/* Make only GUI-broker peers appear foreign, without privileges or changing any real credentials. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <stddef.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>

int
getsockopt(int fd, int level, int option, void *value, socklen_t *length)
{
	int (*original)(int, int, int, void *, socklen_t *) = dlsym(RTLD_NEXT, "getsockopt");
	if (!original) { errno = ENOSYS; return -1; }
	int result = original(fd, level, option, value, length);
	if (result || level != SOL_SOCKET || option != SO_PEERCRED || !value || !length ||
	    *length < sizeof(struct ucred)) return result;
	struct sockaddr_un address = {0};
	socklen_t size = sizeof(address);
	const char *names[] = {"worminal-rust-ui-", "worminal-preview-ui-"};
	if (getsockname(fd, (struct sockaddr *)&address, &size) || address.sun_family != AF_UNIX ||
	    address.sun_path[0]) return result;
	for (size_t i = 0; i < sizeof(names) / sizeof(*names); i++) {
		size_t prefix = strlen(names[i]);
		if (size >= offsetof(struct sockaddr_un, sun_path) + 1 + prefix &&
		    !memcmp(address.sun_path + 1, names[i], prefix)) {
			((struct ucred *)value)->uid ^= 1;
			break;
		}
	}
	return result;
}
