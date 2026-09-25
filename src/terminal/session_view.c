/* Copy terminal cells between the headless owner and an X11 view. */
#include <string.h>
#include "st_state.h"
#include "session_view.h"

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
