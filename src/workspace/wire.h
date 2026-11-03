/* Versioned messages shared by the local socket and SSH stdio bridge. */
#ifndef WORMINAL_WIRE_H
#define WORMINAL_WIRE_H

#include <stddef.h>
#include <stdint.h>
#include "service_policy.h"

#ifdef WORMINAL_PREVIEW_PROTOCOL
#define WIRE_VERSION 2
#define WORKSPACE_ENDPOINT "worminal-workspace-v2"
#else
#define WIRE_VERSION 1
#define WORKSPACE_ENDPOINT "worminal-workspace"
#endif
#ifndef WORKSPACE_DAEMON
#define WORKSPACE_DAEMON "worminald"
#endif
#define WIRE_MAX_BYTES (8u * 1024u * 1024u)
#define WORKSPACE_MAX_COLS 400u
#define WORKSPACE_MAX_ROWS 200u
#define WORKSPACE_MAX_ARGS 1024u
#define WORKSPACE_MAX_ENV 8192u
#define WORKSPACE_INITIAL_CURSOR 2u
/* Rune, attributes, foreground, background: independent of native Glyph padding. */
enum { WIRE_CELL_BYTES = 4u * sizeof(uint32_t) };

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
	WIRE_BROWSER,
	WIRE_BROWSER_NAVIGATE,
	WIRE_ATTACH,
	WIRE_CREATED,
	WIRE_FOCUS_STATE,
	WIRE_CONTROL,
	WIRE_RESIZE,
	WIRE_BROWSER_REPLACE,
	WIRE_COMMAND_DONE,
	WIRE_HANDIN,
	WIRE_HANDOFF,
	WIRE_CAPABILITIES,
	WIRE_COPY_ROWS,
	WIRE_SERVICE_INFO,
	WIRE_SELECT_ALL, /* generation, start/next/cancel; reply: generation, done, UTF-8 bytes. */
};

/* Advertise before HELLO/ATTACH. Older daemons ignore the optional packet;
 * unadvertised clients must continue receiving the original row encoding. */
enum wire_feature {
	WIRE_FEATURE_COPY_ROWS = 1u << 0,
	/* Bit 1 advertised the pre-Rust select-all packet at SERVICE_INFO's current ID.
	 * Keep it reserved so those installed peers cannot negotiate the wrong message. */
	WIRE_FEATURE_SELECT_ALL = 1u << 2
};
enum wire_selection { WIRE_SELECTION_START, WIRE_SELECTION_NEXT, WIRE_SELECTION_CANCEL };

typedef struct {
	unsigned char *data;
	size_t len, cap;
} WireBuffer;

typedef struct {
	uint32_t type, tab;
	unsigned char *data;
	size_t len, pos;
} WirePacket;

typedef struct {
	uint32_t header[4];
	size_t header_bytes, body_bytes;
	WirePacket packet;
} WireReader;

void wire_put_u32(WireBuffer *, uint32_t);
void wire_put_bytes(WireBuffer *, const void *, size_t);
void wire_put_string(WireBuffer *, const char *);
void wire_append_packet(WireBuffer *, uint32_t, uint32_t, const void *, size_t);
int wire_get_u32(WirePacket *, uint32_t *);
int wire_get_bytes(WirePacket *, void *, size_t);
char *wire_get_string(WirePacket *);
int wire_read(int, WirePacket *);
/* Return 1 for a complete packet, 0 for more input needed, -1 for a bad or closed connection. */
int wire_tryread(int, WireReader *, WirePacket *);
void wire_reader_free(WireReader *);
int wire_write(int, uint32_t, uint32_t, const void *, size_t);
int wire_trywrite(int, uint32_t, uint32_t, const void *, size_t);
void wire_packet_free(WirePacket *);
void wire_buffer_free(WireBuffer *);

#endif
