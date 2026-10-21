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
	Term *term = &chosen->term;
	for (int y = 0; y < term->row; y++) {
		free(term->line[y]);
		if (term->alt) free(term->alt[y]);
	}
	free(term->line); free(term->alt);
	if (term->hist)
		for (int y = 0; y < theme_history_size; y++) free(term->hist[y]);
	free(term->hist); free(term->dirty); free(term->tabs);
}

void
tsessionviewresize(TermSession *chosen, int cols, int rows)
{
	Term *term = &chosen->term;
	if (cols < 1 || rows < 1) die("invalid replica dimensions\n");
	TermSession *previous = tsessioncurrent();
	tsessionuse(chosen);
	if ((cols != term->col && !(term->mode & MODE_ALTSCREEN)) ||
	    chosen->sel.ne.x >= cols || chosen->sel.ne.y >= rows) selclear();
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
	created->altscreen_allowed = 0;
	created->term.mode = MODE_WRAP | MODE_UTF8;
	created->term.c.attr = (Glyph){.fg = defaultfg, .bg = defaultbg};
	selinit();
	tsessionviewresize(created, cols, rows);
	return created;
}

SessionFrame
tsessionframe(TermSession *chosen)
{
	Term *term = &chosen->term;
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
	result = tlineat(row, chosen->term.scr);
	tsessionuse(previous);
	return result;
}

int
tsessionrowdirty(TermSession *chosen, int row)
{
	return row >= 0 && row < chosen->term.row && chosen->term.dirty[row];
}

void
tsessioncleandirty(TermSession *chosen)
{
	memset(chosen->term.dirty, 0, chosen->term.row * sizeof(*chosen->term.dirty));
}

void
tsessionimportframe(TermSession *chosen, const SessionFrame *frame)
{
	TermSession *previous = tsessioncurrent();
	Term *term = &chosen->term;
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
	Term *term = &chosen->term;
	if (row < 0 || row >= term->row || cols != term->col)
		die("invalid imported terminal row\n");
	memcpy(term->line[row], glyphs, cols * sizeof(*glyphs));
	term->dirty[row] = 1;
}
