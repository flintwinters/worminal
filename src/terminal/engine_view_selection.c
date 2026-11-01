/* Selection over borrowed imported cells. No parser grid, history, or retained cell pointer. */
#include <stdlib.h>
#include <string.h>
#include "engine_parser.h"

struct WorminalSelection {
	Selection state;
	WorminalFrame frame;
};

static TermEngine
context(WorminalSelection *selection, const WorminalCell *cells)
{
	return (TermEngine){
		.term = {.col = selection->frame.columns, .row = selection->frame.rows,
		         .mode = selection->frame.alternate ? MODE_ALTSCREEN : 0},
		.sel = selection->state, .selection_cells = cells, .selection_only = 1
	};
}

static int
borrow(WorminalSelection *selection, const WorminalCell *cells, size_t capacity, TermEngine *engine)
{
	if (!selection || !cells || !te_dimensions(selection->frame.columns, selection->frame.rows) ||
	    capacity < (size_t)selection->frame.columns * selection->frame.rows) return WORMINAL_INVALID;
	*engine = context(selection, cells);
	return WORMINAL_OK;
}

int
worminal_selection_create(WorminalSelection **result)
{
	if (!result) return WORMINAL_INVALID;
	WorminalSelection *selection = te_alloc(sizeof(*selection));
	memset(selection, 0, sizeof(*selection));
	TermEngine engine = context(selection, NULL);
	te_selinit(&engine);
	selection->state = engine.sel;
	*result = selection;
	return WORMINAL_OK;
}

void worminal_selection_destroy(WorminalSelection *selection) { free(selection); }

int
worminal_selection_configure(WorminalSelection *selection, const WorminalFrame *frame)
{
	if (!selection || !frame || !te_dimensions(frame->columns, frame->rows) || frame->scroll < 0 ||
	    frame->alternate < 0 || frame->alternate > 1 || frame->cursor_x < 0 || frame->cursor_x >= frame->columns ||
	    frame->cursor_y < 0 || frame->cursor_y >= frame->rows) return WORMINAL_INVALID;
	TermEngine engine = context(selection, NULL);
	/* Match legacy imported-view selection invalidation. Main-screen width changes
	 * invalidate wrapped selection coordinates; alternate-screen resizing keeps valid bounds. */
	if ((frame->columns != selection->frame.columns && !selection->frame.alternate) ||
	    engine.sel.ne.x >= frame->columns || engine.sel.ne.y >= frame->rows) te_selclear(&engine);
	selection->state = engine.sel;
	selection->frame = *frame;
	return WORMINAL_OK;
}

int
worminal_selection_clear(WorminalSelection *selection)
{
	if (!selection) return WORMINAL_INVALID;
	TermEngine engine = context(selection, NULL);
	te_selclear(&engine);
	selection->state = engine.sel;
	return WORMINAL_OK;
}

int
worminal_selection_select(WorminalSelection *selection, const WorminalCell *cells, size_t capacity,
                          int32_t column, int32_t row, int32_t start, int32_t snap, int32_t type, int32_t done)
{
	TermEngine engine;
	if (borrow(selection, cells, capacity, &engine) != WORMINAL_OK) return WORMINAL_INVALID;
	int status = worminal_engine_select(&engine, column, row, start, snap, type, done);
	if (status == WORMINAL_OK) selection->state = engine.sel;
	return status;
}

int
worminal_selection_text(WorminalSelection *selection, const WorminalCell *cells, size_t capacity,
                        uint8_t *output, size_t bytes, size_t *required)
{
	TermEngine engine;
	if (borrow(selection, cells, capacity, &engine) != WORMINAL_OK) return WORMINAL_INVALID;
	return worminal_engine_selection(&engine, output, bytes, required);
}

int
worminal_selection_ranges(WorminalSelection *selection, WorminalSelectionRange *ranges,
                          size_t capacity, size_t *required)
{
	if (!selection || !required || (!ranges && capacity) || selection->frame.rows < 1) return WORMINAL_INVALID;
	*required = selection->frame.rows;
	if (capacity < *required) return WORMINAL_CAPACITY;
	TermEngine engine = context(selection, NULL);
	for (int y = 0; y < engine.term.row; y++) {
		WorminalSelectionRange range = {0};
		for (int x = 0; x < engine.term.col; x++) {
			if (!te_selected(&engine, x, y)) continue;
			if (range.first == range.end) range.first = x;
			range.end = x + 1;
		}
		ranges[y] = range;
	}
	return WORMINAL_OK;
}
