/* Native compatibility codec. Its historical layout is shared with retained C daemons. */
#include <stdlib.h>
#include <string.h>
#include "engine_parser.h"
#include "engine_codec.h"
#include WORMINAL_THEME_HEADER
_Static_assert(BUFSIZ == WORMINAL_PENDING_BYTES, "legacy snapshot pending-buffer ABI changed");

typedef struct {
	uint8_t *data;
	size_t size, used;
	int writing, counting;
} Stream;

static int
memory_transfer(void *context, void *data, size_t bytes, int writing)
{
	Stream *stream = context;
	(void)writing;
	if (bytes > SIZE_MAX - stream->used ||
		(!stream->counting && bytes > stream->size - stream->used)) return 0;
	if (bytes && !stream->counting) {
		if (stream->writing) memcpy(stream->data + stream->used, data, bytes);
		else memcpy(data, stream->data + stream->used, bytes);
	}
	stream->used += bytes;
	return 1;
}

static int
transfer(EngineSnapshotIO *io, void *data, size_t bytes, int writing)
{
	return io->transfer(io->context, data, bytes, writing);
}

static int
valid_selection(const EngineCompatSnapshot *header)
{
	const Selection *selection = &header->selection;
	/* Cleared selections retain stale coordinates across resizes; only active bounds are meaningful. */
	if (selection->ob.x == -1) return 1;
	if (selection->mode < SEL_IDLE || selection->mode > SEL_READY ||
	    selection->type < SEL_REGULAR || selection->type > SEL_RECTANGULAR ||
	    selection->snap < 0 || selection->snap > SNAP_LINE ||
	    selection->nb.y > selection->ne.y) return 0;
	const int coordinates[][2] = {{selection->ob.x, selection->ob.y}, {selection->oe.x, selection->oe.y},
	                             {selection->nb.x, selection->nb.y}, {selection->ne.x, selection->ne.y}};
	for (size_t i = 0; i < LEN(coordinates); i++)
		if (coordinates[i][0] < 0 || coordinates[i][0] >= header->col ||
		    coordinates[i][1] < 0 || coordinates[i][1] >= header->row) return 0;
	return 1;
}

int
te_export_compat(TermEngine *s, const WorminalTransportState *transport, EngineSnapshotIO *fd)
{
	Term *t = &s->term;
	if (s->events || transport->pending_bytes < 0 || transport->pending_bytes > WORMINAL_PENDING_BYTES ||
	    transport->output_kind < -1 || transport->output_kind > 1 || s->strescseq.len > 1024 * 1024 ||
	    (size_t)t->col * (t->row * 2 + (size_t)t->histlen) > 16000000) return 0;
	EngineCompatSnapshot h = {
		.magic = 0x57545331, .version = 1, .bytes = sizeof(EngineCompatSnapshot),
		.glyph_bytes = sizeof(Glyph), .history = theme_history_size,
		.row = t->row, .col = t->col, .histlen = t->histlen, .scr = t->scr,
		.c = t->c, .selection = s->sel, .csi = s->csiescseq,
		.ocx = t->ocx, .ocy = t->ocy, .top = t->top, .bot = t->bot,
		.mode = t->mode, .esc = t->esc, .charset = t->charset, .icharset = t->icharset,
		.lastc = t->lastc, .altscreen_allowed = s->altscreen_allowed,
		.ttybuflen = transport->pending_bytes, .remote_scroll = transport->remote_scroll,
		.output_kind = transport->output_kind,
		.str_type = s->strescseq.type, .str_len = s->strescseq.len,
	};
	memcpy(h.saved, s->saved, sizeof(h.saved));
	memcpy(h.trantbl, t->trantbl, sizeof(h.trantbl));
	memcpy(h.ttybuf, transport->pending, sizeof(h.ttybuf));
	if (!transfer(fd, &h, sizeof(h), 1) ||
	    !transfer(fd, t->tabs, t->col * sizeof(int), 1) ||
	    !transfer(fd, s->strescseq.buf, h.str_len, 1)) return 0;
	for (int i = 0; i < t->row * 2 + t->histlen; i++) {
		Line row = i < t->row ? t->line[i] : i < 2 * t->row ? t->alt[i - t->row] :
		    t->hist[(t->histi - t->histlen + i - 2 * t->row + theme_history_size) % theme_history_size];
		if (!transfer(fd, row, t->col * sizeof(Glyph), 1)) return 0;
	}
	return 1;
}

TermEngine *
te_import_compat(EngineSnapshotIO *fd, WorminalTransportState *transport)
{
	EngineCompatSnapshot h;
	TermEngine *s;
	if (!transfer(fd, &h, sizeof(h), 0) || h.magic != 0x57545331 || h.version != 1 ||
	    h.bytes != sizeof(h) || h.glyph_bytes != sizeof(Glyph) ||
	    h.row < 1 || h.row > 4096 || h.col < 1 || h.col > 4096 ||
	    (size_t)h.col * (h.row * 2 + (size_t)MAX(h.histlen, 0)) > 16000000 ||
	    h.histlen < 0 || h.histlen > theme_history_size || h.histlen > (int)h.history ||
	    h.scr < 0 || h.scr > h.histlen || h.top < 0 || h.bot >= h.row || h.bot < h.top ||
	    h.c.x < 0 || h.c.x >= h.col || h.c.y < 0 || h.c.y >= h.row ||
	    h.charset < 0 || h.charset > 3 || h.icharset < 0 || h.icharset > 3 ||
	    h.ttybuflen < 0 || h.ttybuflen > BUFSIZ || h.str_len > 1024 * 1024 ||
	    (h.output_kind < -1 || h.output_kind > 1) || h.csi.len > ESC_BUF_SIZ || h.csi.narg < 0 || h.csi.narg > ESC_ARG_SIZ)
		return NULL;
	/* A pending CSI writes one more byte before testing its limit. Complete CSI buffers may be
	 * full, but no parser-produced pending buffer can reach that state. Selection reads index rows. */
	if (((h.esc & ESC_CSI) && h.csi.len >= ESC_BUF_SIZ - 1) || !valid_selection(&h)) return NULL;
	if (worminal_engine_create(h.col, h.row, &s) != WORMINAL_OK) return NULL;
	Term *t = &s->term;
	t->c = h.c; s->sel = h.selection; s->csiescseq = h.csi;
	t->ocx = h.ocx; t->ocy = h.ocy; t->top = h.top; t->bot = h.bot;
	t->mode = h.mode; t->esc = h.esc; t->charset = h.charset; t->icharset = h.icharset;
	t->lastc = h.lastc; s->altscreen_allowed = h.altscreen_allowed;
	transport->pending_bytes = h.ttybuflen; transport->remote_scroll = h.remote_scroll;
	transport->output_kind = h.output_kind;
	memcpy(s->saved, h.saved, sizeof(h.saved));
	t->c.attr.link = s->saved[0].attr.link = s->saved[1].attr.link = 0;
	memcpy(t->trantbl, h.trantbl, sizeof(h.trantbl));
	memcpy(transport->pending, h.ttybuf, sizeof(h.ttybuf));
	s->strescseq.type = h.str_type;
	s->strescseq.len = h.str_len;
	s->strescseq.siz = MAX(h.str_len + 1, STR_BUF_SIZ);
	s->strescseq.buf = te_realloc(s->strescseq.buf, s->strescseq.siz);
	if (!transfer(fd, t->tabs, t->col * sizeof(int), 0) ||
	    !transfer(fd, s->strescseq.buf, h.str_len, 0)) goto fail;
	s->strescseq.buf[h.str_len] = '\0';
	if (h.histlen) {
		t->hist = calloc(theme_history_size, sizeof(Line));
		if (!t->hist) goto fail;
	}
	for (int i = 0; i < t->row * 2 + h.histlen; i++) {
		Line row;
		if (i < t->row) row = t->line[i];
		else if (i < t->row * 2) row = t->alt[i - t->row];
		else row = t->hist[i - 2 * t->row] = te_alloc(t->col * sizeof(Glyph));
		if (!transfer(fd, row, t->col * sizeof(Glyph), 0)) goto fail;
		/* The URI table is not part of the snapshot: ids (and padding from older owners) are meaningless here. */
		for (int x = 0; x < t->col; x++) row[x].link = 0;
	}
	t->histlen = h.histlen; t->histi = theme_history_size ? h.histlen % theme_history_size : 0;
	t->scr = h.scr;
	for (int i = 0; i < t->row; i++) t->dirty[i] = 1;
	return s;
fail:
	worminal_engine_destroy(s);
	return NULL;
}

int
worminal_engine_export(WorminalEngine *engine, const WorminalTransportState *transport,
					   uint8_t *data, size_t capacity, size_t *required)
{
	if (!engine || !transport || !required || (!data && capacity) ||
		transport->pending_bytes < 0 || transport->pending_bytes > WORMINAL_PENDING_BYTES ||
		transport->output_kind < -1 || transport->output_kind > 1) return WORMINAL_INVALID;
	/* Effects must be applied before ownership can move: snapshots contain terminal state, not callbacks. */
	if (engine->events) return WORMINAL_PENDING;
	Stream counter = {.counting = 1};
	EngineSnapshotIO counter_io = {memory_transfer, &counter};
	if (!te_export_compat(engine, transport, &counter_io)) return WORMINAL_INVALID;
	*required = counter.used;
	if (capacity < counter.used) return WORMINAL_CAPACITY;
	Stream stream = {.data = data, .size = capacity, .writing = 1};
	EngineSnapshotIO io = {memory_transfer, &stream};
	return te_export_compat(engine, transport, &io) ? WORMINAL_OK : WORMINAL_INVALID;
}

int
worminal_engine_import_prefix(const uint8_t *data, size_t bytes, WorminalEngine **result,
					         WorminalTransportState *transport, size_t *consumed)
{
	if (!result) return WORMINAL_INVALID;
	*result = NULL;
	if (!data || !transport || !consumed) return WORMINAL_INVALID;
	*consumed = 0;
	Stream stream = {.data = (uint8_t *)data, .size = bytes};
	WorminalTransportState staged = {0};
	EngineSnapshotIO io = {memory_transfer, &stream};
	TermEngine *engine = te_import_compat(&io, &staged);
	if (!engine) return WORMINAL_INVALID;
	*transport = staged;
	*result = engine;
	*consumed = stream.used;
	return WORMINAL_OK;
}

int
worminal_engine_import(const uint8_t *data, size_t bytes, WorminalEngine **result,
                       WorminalTransportState *transport)
{
	size_t consumed;
	return worminal_engine_import_prefix(data, bytes, result, transport, &consumed);
}
