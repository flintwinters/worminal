/* Private compatibility codec: streaming stays in C adapters, memory buffers in the public engine ABI.
 * The codec never interprets the caller's transport context or owns any descriptor. */
#ifndef WORMINAL_ENGINE_CODEC_H
#define WORMINAL_ENGINE_CODEC_H
#include "engine_state.h"
/* The pending CSI as live handoff transfers it. The layout is frozen: it is CSIEscape as of snapshot version 1
 * (16 arguments, no subparameter mask). Only buf/len are state; csiparse recomputes priv, arguments and mode from
 * buf when the sequence completes, so parser changes must never reach this record. Embedding CSIEscape itself
 * made a daemon with a larger parser reject every older daemon's tabs ("handoff marker was not received"). */
typedef struct {
	char buf[ESC_BUF_SIZ];
	size_t len;
	char priv;
	int arg[16], narg;
	char mode[2];
} SnapshotCSI;
#ifdef __LP64__
_Static_assert(sizeof(SnapshotCSI) == 600, "released daemons exchange this exact pending-CSI layout");
#endif
typedef struct {
	uint32_t magic, version, bytes, glyph_bytes, history;
	int row, col, histlen, scr;
	TCursor c, saved[2];
	Selection selection;
	SnapshotCSI csi;
	int ocx, ocy, top, bot, mode, esc;
	char trantbl[4];
	int charset, icharset;
	Rune lastc;
	int altscreen_allowed, ttybuflen, remote_scroll, output_kind;
	/* Fixed by the snapshot format, not by libc: BUFSIZ is 8192 on glibc but 1024 on musl. */
	char ttybuf[WORMINAL_PENDING_BYTES];
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
