/* Copy terminal cells between the headless owner and a frontend view. */
#include <string.h>
#include <stdlib.h>
#include "st_state.h"
#include "session_view.h"
#include WORMINAL_THEME_HEADER

/* Parser owners keep two screens and history; replicas need only imported cells. */
void
tsessionfreegrid(TermSession *chosen)
{
	te_freegrid(&chosen->engine);
}

void
tsessionviewresize(TermSession *chosen, int cols, int rows)
{
	Term *term = &chosen->engine.term;
	if (cols < 1 || rows < 1) die("invalid replica dimensions\n");
	TermSession *previous = tsessioncurrent();
	tsessionuse(chosen);
	if ((cols != term->col && !(term->mode & MODE_ALTSCREEN)) ||
	    chosen->engine.sel.ne.x >= cols || chosen->engine.sel.ne.y >= rows) selclear();
	for (int y = rows; y < term->row; y++) free(term->line[y]);
	term->line = xrealloc(term->line, rows * sizeof(*term->line));
	term->dirty = xrealloc(term->dirty, rows * sizeof(*term->dirty));
	Glyph blank = {.u = ' ', .fg = defaultfg, .bg = defaultbg};
	for (int y = 0; y < rows; y++) {
		int oldcols = y < term->row ? term->col : 0;
		term->line[y] = xrealloc(oldcols ? term->line[y] : NULL, cols * sizeof(Glyph));
		if (oldcols && cols != oldcols) term->line[y][MIN(cols, oldcols) - 1].mode &= ~ATTR_WRAP;
		for (int x = oldcols; x < cols; x++) term->line[y][x] = blank;
		if (term->line[y][cols - 1].mode & ATTR_WIDE) term->line[y][cols - 1] = blank;
		term->dirty[y] = 1;
	}
	term->col = cols; term->row = rows; term->top = 0; term->bot = rows - 1;
	LIMIT(term->c.x, 0, cols - 1); LIMIT(term->c.y, 0, rows - 1);
	tsessionuse(previous);
}

TermSession *
tsessionviewnew(int cols, int rows)
{
	TermSession *created = tsessionalloc();
	created->view_only = 1;
	created->engine.altscreen_allowed = 0;
	created->engine.term.mode = MODE_WRAP | MODE_UTF8;
	created->engine.term.c.attr = (Glyph){.fg = defaultfg, .bg = defaultbg};
	selinit();
	tsessionviewresize(created, cols, rows);
	return created;
}

SessionFrame
tsessionframe(TermSession *chosen)
{
	Term *term = &chosen->engine.term;
	return (SessionFrame) {
		.cols = term->col, .rows = term->row,
		.cursor_x = term->c.x, .cursor_y = term->c.y,
		.scroll = term->scr, .alternate = !!(term->mode & MODE_ALTSCREEN)
	};
}

Line
tsessionviewline(TermSession *chosen, int row)
{
	TermSession *previous = tsessioncurrent();
	Line result;
	tsessionuse(chosen);
	result = tlineat(row, chosen->engine.term.scr);
	tsessionuse(previous);
	return result;
}

int
tsessionrowdirty(TermSession *chosen, int row)
{
	return row >= 0 && row < chosen->engine.term.row && chosen->engine.term.dirty[row];
}

void
tsessioncleandirty(TermSession *chosen)
{
	memset(chosen->engine.term.dirty, 0, chosen->engine.term.row * sizeof(*chosen->engine.term.dirty));
}

void
tsessionimportframe(TermSession *chosen, const SessionFrame *frame)
{
	TermSession *previous = tsessioncurrent();
	Term *term = &chosen->engine.term;
	tsessionuse(chosen);
	if (term->col != frame->cols || term->row != frame->rows)
		tresize(frame->cols, frame->rows);
	term->c.x = frame->cursor_x;
	term->c.y = frame->cursor_y;
	term->scr = 0;
	chosen->remote_scroll = frame->scroll;
	if (frame->alternate)
		term->mode |= MODE_ALTSCREEN;
	else
		term->mode &= ~MODE_ALTSCREEN;
	tsessionuse(previous);
}

void
tsessionimportrow(TermSession *chosen, int row, const Glyph *glyphs, int cols)
{
	Term *term = &chosen->engine.term;
	if (row < 0 || row >= term->row || cols != term->col)
		die("invalid imported terminal row\n");
	memcpy(term->line[row], glyphs, cols * sizeof(*glyphs));
	term->dirty[row] = 1;
}

/* Imported row moves have memmove semantics, including overlapping ranges. */
void
tsessioncopyrows(TermSession *chosen, int source, int destination, int count)
{
	Term *term = &chosen->engine.term;
	if (count < 1 || count > term->row || source < 0 || destination < 0 ||
	    source > term->row - count || destination > term->row - count)
		die("invalid imported row move\n");
	for (int step = 0; step < count; step++) {
		int offset = destination > source ? count - 1 - step : step;
		memmove(term->line[destination + offset], term->line[source + offset], term->col * sizeof(Glyph));
		term->dirty[destination + offset] = 1;
	}
}
