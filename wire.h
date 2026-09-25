/* Versioned messages shared by the local socket and SSH stdio bridge. */
#ifndef WORMINAL_WIRE_H
#define WORMINAL_WIRE_H

#include <stddef.h>
#include <stdint.h>

#define WIRE_VERSION 1
#define WIRE_MAX_BYTES (8u * 1024u * 1024u)

enum wire_type {
	WIRE_HELLO = 1,
	WIRE_NEW,
	WIRE_INPUT,
	WIRE_FOCUS,
	WIRE_CLOSE,
	WIRE_SCROLL,
	WIRE_CATALOG,
	WIRE_FRAME,
	WIRE_ROW,
	WIRE_FRAME_FINISH,
	WIRE_CLIPBOARD,
	WIRE_BELL,
	WIRE_STOP,
	WIRE_PRINT,
	WIRE_RELEASE,
	WIRE_ERROR,
};

typedef struct {
	unsigned char *data;
	size_t len, cap;
} WireBuffer;

typedef struct {
	uint32_t type, tab;
	unsigned char *data;
	size_t len, pos;
} WirePacket;

void wire_put_u32(WireBuffer *, uint32_t);
void wire_put_bytes(WireBuffer *, const void *, size_t);
void wire_put_string(WireBuffer *, const char *);
void wire_append_packet(WireBuffer *, uint32_t, uint32_t, const void *, size_t);
int wire_get_u32(WirePacket *, uint32_t *);
int wire_get_bytes(WirePacket *, void *, size_t);
char *wire_get_string(WirePacket *);
int wire_read(int, WirePacket *);
int wire_write(int, uint32_t, uint32_t, const void *, size_t);
int wire_trywrite(int, uint32_t, uint32_t, const void *, size_t);
void wire_packet_free(WirePacket *);
void wire_buffer_free(WireBuffer *);

#endif
