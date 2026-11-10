/* See LICENSE. Public, instance-based boundary around the shared terminal parser. */
#include <limits.h>
#include <stdlib.h>
#include <string.h>
#include "engine_parser.h"
#include WORMINAL_THEME_HEADER

int
te_dimensions(int32_t columns, int32_t rows)
{
	return columns > 0 && columns <= 4096 && rows > 0 && rows <= 4096 &&
	       (size_t)columns * rows <= 16000000;
}

int
worminal_engine_create(int32_t columns, int32_t rows, WorminalEngine **result)
{
	if (!result) return WORMINAL_INVALID;
	*result = NULL;
	if (!te_dimensions(columns, rows)) return WORMINAL_INVALID;
	TermEngine *created = te_alloc(sizeof(*created));
	memset(created, 0, sizeof(*created));
	created->altscreen_allowed = 1;
	te_tnew(created, columns, rows);
	te_selinit(created);
	*result = created;
	return WORMINAL_OK;
}

void
te_freegrid(TermEngine *engine)
{
	Term *grid = &engine->term;
	for (int y = 0; y < grid->row; y++) {
		free(grid->line[y]);
		if (grid->alt) free(grid->alt[y]);
	}
	free(grid->line); free(grid->alt);
	if (grid->hist)
		for (int y = 0; y < theme_history_size; y++) free(grid->hist[y]);
	free(grid->hist); free(grid->dirty); free(grid->tabs);
	memset(grid, 0, sizeof(*grid));
}

void
worminal_engine_destroy(WorminalEngine *engine)
{
	if (!engine) return;
	te_freegrid(engine);
	te_freeevents(engine);
	free(engine->strescseq.buf);
	te_links_free(engine);
	free(engine);
}

int
worminal_engine_feed(WorminalEngine *engine, const uint8_t *input, size_t bytes,
					int64_t now, size_t *consumed)
{
	if (!engine || (!input && bytes) || !consumed || bytes > INT_MAX || now < 0)
		return WORMINAL_INVALID;
	*consumed = 0;
	if (engine->events) return WORMINAL_PENDING;
	if (now < engine->now) return WORMINAL_INVALID;
	engine->now = now;
	*consumed = te_twrite(engine, (const char *)input, bytes, 0);
	return WORMINAL_OK;
}

int
worminal_engine_input(WorminalEngine *engine, const uint8_t *input, size_t bytes, int32_t echo, int64_t now)
{
	if (!engine || (!input && bytes) || bytes > INT_MAX || now < 0 || now < engine->now ||
	    (echo != 0 && echo != 1)) return WORMINAL_INVALID;
	if (engine->events) return WORMINAL_PENDING;
	engine->now = now;
	Term *grid = &engine->term;
	if (grid->scr) {
		te_selclear(engine);
		grid->scr = 0;
		te_tfulldirt(engine);
	}
	if (echo && (grid->mode & MODE_ECHO)) {
		size_t offset = 0;
		while (offset < bytes) {
			int used = te_twrite(engine, (const char *)input + offset, bytes - offset, 1);
			if (!used) break;
			offset += used;
		}
	}
	/* Terminal LNM matches the legacy ONLCR-style CR expansion. No process state is involved. */
	if (!(grid->mode & MODE_CRLF)) {
		if (bytes) te_emit(engine, WORMINAL_WRITE, 0, 0, input, bytes);
		return WORMINAL_OK;
	}
	while (bytes) {
		size_t used;
		if (*input == '\r') {
			te_emit(engine, WORMINAL_WRITE, 0, 0, "\r\n", 2);
			used = 1;
		} else {
			const uint8_t *next = memchr(input, '\r', bytes);
			used = next ? (size_t)(next - input) : bytes;
			te_emit(engine, WORMINAL_WRITE, 0, 0, input, used);
		}
		input += used;
		bytes -= used;
	}
	return WORMINAL_OK;
}

int
worminal_engine_resize(WorminalEngine *engine, int32_t columns, int32_t rows)
{
	if (!engine || !te_dimensions(columns, rows)) return WORMINAL_INVALID;
	te_tresize(engine, columns, rows);
	return WORMINAL_OK;
}

int
worminal_engine_scroll(WorminalEngine *engine, int32_t up, int32_t rows)
{
	if (!engine || (up != 0 && up != 1)) return WORMINAL_INVALID;
	Arg amount = {.i = rows};
	if (up) te_kscrollup(engine, &amount); else te_kscrolldown(engine, &amount);
	return WORMINAL_OK;
}

int
worminal_engine_select(WorminalEngine *engine, int32_t column, int32_t row,
					  int32_t start, int32_t snap, int32_t type, int32_t done)
{
	if (!engine || column < 0 || column >= engine->term.col || row < 0 || row >= engine->term.row ||
	    snap < 0 || snap > SNAP_LINE || type < SEL_REGULAR || type > SEL_RECTANGULAR ||
	    (start != 0 && start != 1) || (done != 0 && done != 1)) return WORMINAL_INVALID;
	if (start) te_selstart(engine, column, row, snap);
	else te_selextend(engine, column, row, type, done);
	return WORMINAL_OK;
}

int
worminal_engine_clear_selection(WorminalEngine *engine)
{
	if (!engine) return WORMINAL_INVALID;
	te_selclear(engine);
	return WORMINAL_OK;
}

int
worminal_engine_selected(WorminalEngine *engine, int32_t column, int32_t row, int32_t *selected)
{
	if (!engine || !selected || column < 0 || column >= engine->term.col ||
	    row < 0 || row >= engine->term.row) return WORMINAL_INVALID;
	*selected = !!te_selected(engine, column, row);
	return WORMINAL_OK;
}

int
worminal_engine_allow_alternate(WorminalEngine *engine, int32_t allowed)
{
	if (!engine || (allowed != 0 && allowed != 1)) return WORMINAL_INVALID;
	engine->altscreen_allowed = allowed;
	return WORMINAL_OK;
}

int
worminal_engine_control(WorminalEngine *engine, uint32_t operation)
{
	if (!engine || operation > WORMINAL_PRINT_SCREEN) return WORMINAL_INVALID;
	if (engine->events) return WORMINAL_PENDING;
	if (operation == WORMINAL_TOGGLE_PRINTER) te_toggleprinter(engine, NULL);
	else te_printscreen(engine, NULL);
	return WORMINAL_OK;
}

/* Copies selected text. Trimming drops the breaks of blank trailing rows but keeps the one that
 * terminates the last nonblank line. */
static int
copy_selection(WorminalEngine *engine, uint8_t *output, size_t capacity, size_t *bytes, int trim)
{
	if (!engine || !bytes || (!output && capacity)) return WORMINAL_INVALID;
	char *text = te_getsel(engine);
	*bytes = text ? strlen(text) : 0;
	if (trim) {
		while (*bytes && text[*bytes - 1] == '\n') --*bytes;
		if (*bytes) ++*bytes;
	}
	int result = *bytes > capacity ? WORMINAL_CAPACITY : WORMINAL_OK;
	if (*bytes && result == WORMINAL_OK) memcpy(output, text, *bytes);
	free(text);
	return result;
}

int
worminal_engine_selection(WorminalEngine *engine, uint8_t *output, size_t capacity, size_t *bytes)
{
	return copy_selection(engine, output, capacity, bytes, 0);
}

int
worminal_engine_all_text(WorminalEngine *engine, uint8_t *output, size_t capacity, size_t *bytes)
{
	if (!engine) return WORMINAL_INVALID;
	Selection saved = engine->sel;
	int scroll = engine->term.scr;
	int first = engine->term.mode & MODE_ALTSCREEN ? 0 : -engine->term.histlen;
	engine->term.scr = 0;
	engine->sel = (Selection){.type = SEL_REGULAR,
	    .ob = {0, first}, .nb = {0, first},
	    .oe = {engine->term.col - 1, engine->term.row - 1},
	    .ne = {engine->term.col - 1, engine->term.row - 1}};
	/* The selection always ends at the last screen row, so rows below the output are blank. */
	int status = copy_selection(engine, output, capacity, bytes, 1);
	engine->sel = saved;
	engine->term.scr = scroll;
	return status;
}

int
worminal_engine_frame(WorminalEngine *engine, int64_t now, WorminalFrame *frame)
{
	if (!engine || !frame || now < engine->now || now < 0) return WORMINAL_INVALID;
	te_renderdelay(engine, now);
	Term *grid = &engine->term;
	*frame = (WorminalFrame){grid->col, grid->row, grid->c.x, grid->c.y, grid->scr,
	                      !!(grid->mode & MODE_ALTSCREEN), engine->render_sync_deadline};
	return WORMINAL_OK;
}

int
worminal_engine_row(WorminalEngine *engine, int32_t row, WorminalCell *cells,
				   size_t capacity, size_t *required, int32_t *dirty)
{
	if (!engine || row < 0 || row >= engine->term.row || !required || !dirty || (!cells && capacity))
		return WORMINAL_INVALID;
	*required = engine->term.col;
	*dirty = !!engine->term.dirty[row];
	if (capacity < *required) return WORMINAL_CAPACITY;
	Line source = te_tline(engine, row);
	for (size_t x = 0; x < *required; x++)
		cells[x] = (WorminalCell){source[x].u, source[x].mode, source[x].fg, source[x].bg};
	return WORMINAL_OK;
}

int
worminal_engine_row_links(WorminalEngine *engine, int32_t row, uint16_t *ids, size_t capacity,
				   size_t *required)
{
	if (!engine || row < 0 || row >= engine->term.row || !required || (!ids && capacity))
		return WORMINAL_INVALID;
	*required = engine->term.col;
	if (capacity < *required) return WORMINAL_CAPACITY;
	Line source = te_tline(engine, row);
	for (size_t x = 0; x < *required; x++) ids[x] = source[x].link;
	return WORMINAL_OK;
}

const char *
worminal_engine_link_uri(WorminalEngine *engine, uint16_t id)
{
	return engine && id && id < engine->link_count ? engine->links[id] : NULL;
}

int
worminal_engine_clean(WorminalEngine *engine)
{
	if (!engine) return WORMINAL_INVALID;
	/* A held redraw must retain damage until completion or caller-time expiration. */
	if (engine->render_sync_deadline) return WORMINAL_PENDING;
	memset(engine->term.dirty, 0, engine->term.row * sizeof(*engine->term.dirty));
	return WORMINAL_OK;
}
