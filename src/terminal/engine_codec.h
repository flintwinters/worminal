/* Private compatibility codec: streaming stays in C adapters, memory buffers in the public engine ABI.
 * The codec never interprets the caller's transport context or owns any descriptor. */
#ifndef WORMINAL_ENGINE_CODEC_H
#define WORMINAL_ENGINE_CODEC_H
#include "engine_state.h"
typedef struct {
	uint32_t magic, version, bytes, glyph_bytes, history;
	int row, col, histlen, scr;
	TCursor c, saved[2];
	Selection selection;
	CSIEscape csi;
	int ocx, ocy, top, bot, mode, esc;
	char trantbl[4];
	int charset, icharset;
	Rune lastc;
	int altscreen_allowed, ttybuflen, remote_scroll, output_kind;
	char ttybuf[BUFSIZ];
	char str_type;
	uint32_t str_len;
} EngineCompatSnapshot;


typedef struct {
	int (*transfer)(void *, void *, size_t, int);
	void *context;
} EngineSnapshotIO;
int te_export_compat(TermEngine *, const WorminalTransportState *, EngineSnapshotIO *);
TermEngine *te_import_compat(EngineSnapshotIO *, WorminalTransportState *);
#endif
