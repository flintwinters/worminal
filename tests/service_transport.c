/* Record transport flush boundaries without changing daemon wire traffic. */
#define _GNU_SOURCE
#include <arpa/inet.h>
#include <dlfcn.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>

ssize_t
send(int fd, const void *bytes, size_t length, int flags)
{
	ssize_t (*original)(int, const void *, size_t, int) = dlsym(RTLD_NEXT, "send");
	const char *path = getenv("WORMINAL_TEST_SEND_LOG");
	if (path) {
		size_t offset = 0;
		uint32_t frames = 0, unfinished = 0, last = 0;
		while (length - offset >= 16) {
			uint32_t header[4];
			memcpy(header, (const unsigned char *)bytes + offset, sizeof(header));
			size_t body = ntohl(header[3]);
			if (body > length - offset - sizeof(header)) break;
			last = ntohl(header[1]);
			/* A write may batch CREATED/CATALOG before a frame or metadata after it.
			 * Every frame started in this small-frame fixture must finish in the batch. */
			if (last == 8) { frames++; unfinished++; }
			if (last == 10 && unfinished) unfinished--;
			offset += sizeof(header) + body;
		}
		FILE *log = fopen(path, "a");
		if (log) {
			fprintf(log, "%u %u %zu\n", frames ? 8 : 0, unfinished ? 0 : 10, length);
			fclose(log);
		}
	}
	return original(fd, bytes, length, flags);
}
