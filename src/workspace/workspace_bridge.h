/* Bounded, independent forwarding for the SSH stdio bridge. */
#ifndef WORMINAL_WORKSPACE_BRIDGE_H
#define WORMINAL_WORKSPACE_BRIDGE_H

#include <errno.h>
#include <fcntl.h>
#include <sys/select.h>
#include <unistd.h>

static void
workspace_bridge(int fd)
{
	struct {
		int source, target;
		unsigned char bytes[16384];
		size_t length, sent;
	} streams[2] = {{.source = 0, .target = fd}, {.source = fd, .target = 1}};
	int descriptors[] = {0, 1, fd};
	for (size_t i = 0; i < sizeof(descriptors) / sizeof(*descriptors); i++) {
		int flags = fcntl(descriptors[i], F_GETFL);
		if (flags < 0 || fcntl(descriptors[i], F_SETFL, flags | O_NONBLOCK) < 0)
			return;
	}
	/* SSH can stop reading stdout when its channel window fills. Never let
	 * that wait prevent incoming keyboard input from reaching the daemon.
	 * One fixed buffer per direction also bounds bridge read-ahead. */
	for (;;) {
		fd_set readers, writers;
		FD_ZERO(&readers);
		FD_ZERO(&writers);
		for (int i = 0; i < 2; i++) {
			if (!streams[i].length) FD_SET(streams[i].source, &readers);
			else FD_SET(streams[i].target, &writers);
		}
		if (select(fd + 1, &readers, &writers, NULL, NULL) < 0) {
			if (errno == EINTR) continue;
			return;
		}
		for (int i = 0; i < 2; i++) {
			ssize_t count;
			if (!streams[i].length && FD_ISSET(streams[i].source, &readers)) {
				count = read(streams[i].source, streams[i].bytes, sizeof(streams[i].bytes));
				if (count > 0) streams[i].length = count;
				else if (!count || (errno != EINTR && errno != EAGAIN && errno != EWOULDBLOCK))
					return;
			} else if (streams[i].length && FD_ISSET(streams[i].target, &writers)) {
				count = write(streams[i].target, streams[i].bytes + streams[i].sent,
				              streams[i].length - streams[i].sent);
				if (count > 0) {
					streams[i].sent += count;
					if (streams[i].sent == streams[i].length)
						streams[i].length = streams[i].sent = 0;
				} else if (!count || (errno != EINTR && errno != EAGAIN && errno != EWOULDBLOCK))
					return;
			}
		}
	}
}
#endif
