#include <arpa/inet.h>
#include <errno.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>
#include "wire.h"

static void
wire_reserve(WireBuffer *buffer, size_t extra)
{
	size_t needed = buffer->len + extra;
	if (needed > WIRE_MAX_BYTES)
		abort();
	if (needed <= buffer->cap)
		return;
	size_t cap = buffer->cap ? buffer->cap : 256;
	while (cap < needed)
		cap *= 2;
	unsigned char *data = realloc(buffer->data, cap);
	if (!data)
		abort();
	buffer->data = data;
	buffer->cap = cap;
}

void
wire_put_bytes(WireBuffer *buffer, const void *bytes, size_t length)
{
	wire_reserve(buffer, length);
	if (length)
		memcpy(buffer->data + buffer->len, bytes, length);
	buffer->len += length;
}

void
wire_put_u32(WireBuffer *buffer, uint32_t value)
{
	value = htonl(value);
	wire_put_bytes(buffer, &value, sizeof(value));
}

void
wire_put_string(WireBuffer *buffer, const char *value)
{
	size_t length = value ? strlen(value) : 0;
	wire_put_u32(buffer, length);
	wire_put_bytes(buffer, value ? value : "", length);
}

void
wire_append_packet(WireBuffer *buffer, uint32_t type, uint32_t tab,
                   const void *data, size_t length)
{
	uint32_t header[4] = {htonl(WIRE_VERSION), htonl(type), htonl(tab), htonl(length)};
	wire_put_bytes(buffer, header, sizeof(header));
	wire_put_bytes(buffer, data, length);
}

int
wire_get_bytes(WirePacket *packet, void *destination, size_t length)
{
	if (length > packet->len - packet->pos)
		return 0;
	memcpy(destination, packet->data + packet->pos, length);
	packet->pos += length;
	return 1;
}

int
wire_get_u32(WirePacket *packet, uint32_t *value)
{
	uint32_t encoded;
	if (!wire_get_bytes(packet, &encoded, sizeof(encoded)))
		return 0;
	*value = ntohl(encoded);
	return 1;
}

char *
wire_get_string(WirePacket *packet)
{
	uint32_t length;
	char *result;
	if (!wire_get_u32(packet, &length) || length > packet->len - packet->pos)
		return NULL;
	result = malloc((size_t)length + 1);
	if (!result)
		abort();
	memcpy(result, packet->data + packet->pos, length);
	result[length] = '\0';
	packet->pos += length;
	return result;
}

static int
wire_exact(int fd, void *bytes, size_t length)
{
	unsigned char *cursor = bytes;
	while (length) {
		ssize_t count = read(fd, cursor, length);
		if (count < 0 && errno == EINTR)
			continue;
		if (count <= 0)
			return 0;
		cursor += count;
		length -= count;
	}
	return 1;
}

int
wire_read(int fd, WirePacket *packet)
{
	uint32_t header[4];
	memset(packet, 0, sizeof(*packet));
	if (!wire_exact(fd, header, sizeof(header)))
		return 0;
	if (ntohl(header[0]) != WIRE_VERSION || ntohl(header[3]) > WIRE_MAX_BYTES)
		return 0;
	packet->type = ntohl(header[1]);
	packet->tab = ntohl(header[2]);
	packet->len = ntohl(header[3]);
	packet->data = malloc(packet->len ? packet->len : 1);
	if (!packet->data)
		abort();
	if (!wire_exact(fd, packet->data, packet->len)) {
		wire_packet_free(packet);
		return 0;
	}
	return 1;
}

static int
wire_output(int fd, uint32_t type, uint32_t tab, const void *data, size_t len, int fast)
{
	uint32_t header[4] = {htonl(WIRE_VERSION), htonl(type), htonl(tab), htonl(len)};
	unsigned char *bytes;
	size_t size = sizeof(header) + len, sent = 0;
	if (len > WIRE_MAX_BYTES)
		return 0;
	bytes = malloc(size);
	if (!bytes)
		abort();
	memcpy(bytes, header, sizeof(header));
	if (len)
		memcpy(bytes + sizeof(header), data, len);
	while (sent < size) {
		ssize_t count = send(fd, bytes + sent, size - sent,
		                     MSG_NOSIGNAL | (fast ? MSG_DONTWAIT : 0));
		if (count < 0 && errno == EINTR)
			continue;
		if (count <= 0)
			break;
		sent += count;
	}
	free(bytes);
	return sent == size;
}

int wire_write(int fd, uint32_t type, uint32_t tab, const void *data, size_t len)
{
	return wire_output(fd, type, tab, data, len, 0);
}

int wire_trywrite(int fd, uint32_t type, uint32_t tab, const void *data, size_t len)
{
	return wire_output(fd, type, tab, data, len, 1);
}

void wire_packet_free(WirePacket *packet) { free(packet->data); }
void wire_buffer_free(WireBuffer *buffer) { free(buffer->data); }
